//! 3.3.5a's currency list: `CurrencyCategory.dbc`'s headers, each over its known
//! `CurrencyTypes.dbc` rows, the rows marked unused gathered under the category that sorts last,
//! and the backpack's watched tokens. The unused and watched marks are the per-character
//! `currencyTokens*` CVars, one bit per currency's `BitIndex` (`0x5b0440`, `0x5b0590`).

use mlua::{Lua, MultiValue, Table, Value};

use super::who_sort::sort_utf8i;

/// Honor and arena points, counted from the player's fields, not a token slot (`0x5b0757`).
const ARENA_POINTS_ITEM: u32 = 43307;
const HONOR_POINTS_ITEM: u32 = 43308;
/// The display list's capacity (95 entries of 24 bytes, indexed at `0x5b06dd`).
const MAX_ENTRIES: usize = 95;

/// One `CurrencyTypes.dbc` row.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CurrencyTypeView {
    pub item_id: u32,
    pub category_id: u32,
    /// The known-currencies and CVar bit, 1-based.
    pub bit_index: i32,
}

/// One `CurrencyCategory.dbc` row.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CurrencyCategoryView {
    pub id: u32,
    pub flags: u32,
    pub name: String,
}

/// Both tables in file order, and each currency item's cached name and icon.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CurrencyCatalogView {
    pub types: Vec<CurrencyTypeView>,
    pub categories: Vec<CurrencyCategoryView>,
}

/// The player's side: the known bits, the token slots' counts and the two point balances.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CurrencyHoldings {
    /// `PLAYER_FIELD_KNOWN_CURRENCIES`, bit `BitIndex - 1`.
    pub known: u64,
    /// Each token slot's item entry and stack count.
    pub tokens: Vec<(u32, u32)>,
    pub honor: u32,
    pub arena: u32,
    /// Each currency item's cached name and icon path, `None` until the item query answers.
    pub items: std::collections::HashMap<u32, (Option<String>, Option<String>)>,
}

/// What a `ReloadUI` carries into the new VM, as these live outside the Lua state: the categories' open flags, set only at a world entry from the character screen (`0x5afd90`), the display list as its last build left it, and the records its builds still wait on.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CurrencyFolds {
    collapsed: std::collections::HashMap<u32, bool>,
    list: Vec<Entry>,
    waiting: std::collections::HashSet<u32>,
}

/// One display-list entry (24 bytes, indexed at `0x5b06dd`).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Entry {
    name: Option<String>,
    item_id: u32,
    category_id: u32,
    header: bool,
    expanded: bool,
    watched: bool,
}

impl super::UiScript {
    /// Push the token slots' item views, slot 1 being player slot `0x76`; `None` with no player.
    pub fn set_token_slots(&mut self, slots: Option<super::ContainerState>) {
        set_field!(self, token_slots, slots);
    }

    /// The display list build (`0x5b0030`) over the live tables and our currencies, as each of 12340's triggers runs it before its event (`0x5b0640`, `0x5b0660`).
    pub fn rebuild_currency_list(&mut self) {
        build(&mut self.model_mut());
    }

    /// A record a build asked for landed, answered or denied: whether a build waited on it, so its callback rebuilds and fires `CURRENCY_DISPLAY_UPDATE` (`0x5b01d7`, `0x5b0260` -> `0x5b0660`).
    pub fn currency_record_landed(&mut self, item_id: u32) -> bool {
        let waited = self.model_mut().currency_waiting.remove(&item_id);
        if waited {
            self.rebuild_currency_list();
        }
        waited
    }

    /// Take what outlives a `ReloadUI` out of this VM.
    pub fn take_currency_folds(&mut self) -> CurrencyFolds {
        let mut model = self.model_mut();
        CurrencyFolds {
            collapsed: std::mem::take(&mut model.currency_collapsed),
            list: std::mem::take(&mut model.currency_list),
            waiting: std::mem::take(&mut model.currency_waiting),
        }
    }

    /// Put what was taken from the previous VM back.
    pub fn restore_currency_folds(&mut self, folds: CurrencyFolds) {
        let mut model = self.model_mut();
        model.currency_collapsed = folds.collapsed;
        model.currency_list = folds.list;
        model.currency_waiting = folds.waiting;
    }
}

fn read(lua: &Lua) -> mlua::AppDataRef<'_, super::ModelCell> {
    lua.app_data_ref::<super::ModelCell>()
        .expect("model app_data")
}

/// The live currency tables through the lent view, `None` with none lent or before they load.
fn with_catalog<R>(f: impl FnOnce(&CurrencyCatalogView) -> R) -> Option<R> {
    super::read_live(|l| l.currency_catalog().map(f)).flatten()
}

/// Our currencies at the call, `None` with no active player.
fn holdings() -> Option<CurrencyHoldings> {
    super::read_live(|l| l.currency_holdings()).flatten()
}

/// A per-character CVar as an integer, 0 when unregistered.
fn cvar_int(model: &super::Model, name: &str) -> u32 {
    model
        .cvars
        .get(&name.to_ascii_lowercase())
        .map_or(0, |c| super::cvars::sstr_to_int(&c.value) as u32)
}

/// Whether `bit` (1-based) is set in the pair `name1`/`name2` (`0x5b04f0`).
fn cvar_bit(model: &super::Model, pair: &str, bit: i32) -> bool {
    match bit {
        1..=32 => cvar_int(model, &format!("{pair}1")) & (1 << (bit - 1)) != 0,
        33..=64 => cvar_int(model, &format!("{pair}2")) & (1 << (bit - 33)) != 0,
        _ => false,
    }
}

fn known(h: Option<&CurrencyHoldings>, bit: i32) -> bool {
    let known = h.map_or(0, |h| h.known);
    (1..=64).contains(&bit) && known & (1 << (bit - 1)) != 0
}

/// The categories in display order (`0x5afc40`): unflagged first, then by the UTF-8 sort (`0x5afc70`).
fn sorted_categories(catalog: &CurrencyCatalogView) -> Vec<&CurrencyCategoryView> {
    let mut cats: Vec<&CurrencyCategoryView> = catalog.categories.iter().collect();
    cats.sort_by(|a, b| {
        (a.flags & 1)
            .cmp(&(b.flags & 1))
            .then_with(|| sort_utf8i(&a.name, &b.name))
    });
    cats
}

/// The display list build (`0x5b0030`): a header per category with a row to show, then, expanded,
/// its known rows not marked unused, or under the last category every row marked unused; each listed row's uncached record is asked for with the landing callback (`0x5b01d7`, `0x5b0260`).
fn build(model: &mut super::Model) {
    let h = holdings();
    let list = with_catalog(|c| display_list(model, c, h.as_ref())).unwrap_or_default();
    for e in list.iter().filter(|e| !e.header && e.name.is_none()) {
        super::item_stats::wait_item_record(model, e.item_id, super::ItemRecordWait::Currency);
        super::read_live(|l| l.ask_item_record(e.item_id));
        model.currency_waiting.insert(e.item_id);
    }
    model.currency_list = list;
}

fn display_list(
    model: &super::Model,
    catalog: &CurrencyCatalogView,
    h: Option<&CurrencyHoldings>,
) -> Vec<Entry> {
    let cats = sorted_categories(catalog);
    let types = &catalog.types;
    let name_of = |item: u32| {
        h.and_then(|h| h.items.get(&item))
            .and_then(|(n, _)| n.clone())
    };
    let mut out = Vec::new();
    for (i, cat) in cats.iter().enumerate() {
        let last = i + 1 == cats.len();
        // The category's known-row test (`0x5afe30`): a known row of the category, or for the last
        // one any row marked unused, known or not.
        let has_known = types.iter().any(|t| {
            (t.category_id == cat.id && known(h, t.bit_index))
                || (last && cvar_bit(model, "currencyTokensUnused", t.bit_index))
        });
        if !has_known {
            continue;
        }
        let expanded = model.currency_collapsed.get(&cat.id).is_none_or(|c| !*c);
        out.push(Entry {
            name: Some(cat.name.clone()),
            item_id: 0,
            category_id: cat.id,
            header: true,
            expanded,
            watched: false,
        });
        if !expanded {
            continue;
        }
        let mut rows: Vec<Entry> = types
            .iter()
            .filter(|t| known(h, t.bit_index))
            .filter(|t| {
                let unused = cvar_bit(model, "currencyTokensUnused", t.bit_index);
                if last {
                    unused
                } else {
                    t.category_id == cat.id && !unused
                }
            })
            .map(|t| Entry {
                name: name_of(t.item_id),
                item_id: t.item_id,
                category_id: t.category_id,
                header: false,
                expanded: false,
                watched: cvar_bit(model, "currencyTokensBackpack", t.bit_index),
            })
            .collect();
        // The row sort (`0x5aff10`): the UTF-8 sort (`0x5affbc`), uncached names last.
        rows.sort_by(|a, b| match (&a.name, &b.name) {
            (Some(x), Some(y)) => sort_utf8i(x, y),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        });
        out.extend(rows);
    }
    out.truncate(MAX_ENTRIES);
    out
}

/// A currency's count and kind (`0x5b0757`): arena 1, honor 2, a token 0; `None` with no player.
fn count_of(h: Option<&CurrencyHoldings>, item_id: u32) -> Option<(u32, u32)> {
    let h = h?;
    Some(match item_id {
        ARENA_POINTS_ITEM => (h.arena, 1),
        HONOR_POINTS_ITEM => (h.honor, 2),
        id => (
            h.tokens
                .iter()
                .find(|(entry, _)| *entry == id)
                .map_or(0, |&(_, n)| n),
            0,
        ),
    })
}

/// The cached record's icon, read at the call (`0x5b0859`).
fn icon_of(h: Option<&CurrencyHoldings>, item_id: u32) -> Option<String> {
    h.and_then(|h| h.items.get(&item_id))
        .and_then(|(_, icon)| icon.clone())
}

/// The `i`-th currency (0-based) in file order both known and watched (`0x5b0340`).
fn backpack_currency(
    model: &super::Model,
    h: Option<&CurrencyHoldings>,
    i: usize,
) -> Option<CurrencyTypeView> {
    with_catalog(|c| {
        c.types
            .iter()
            .filter(|t| {
                known(h, t.bit_index) && cvar_bit(model, "currencyTokensBackpack", t.bit_index)
            })
            .nth(i)
            .copied()
    })
    .flatten()
}

/// A 1-based list index, `lua_isnumber` gated (`0x5b0680`), made 0-based.
fn index_arg(lua: &Lua, v: Value, usage: &'static str) -> mlua::Result<i64> {
    match lua.coerce_number(v)? {
        Some(n) => Ok(i64::from(super::binding_abi::ftol(n)) - 1),
        None => Err(mlua::Error::runtime(usage)),
    }
}

/// The unused and backpack setters (`0x5b0440`, `0x5b0590`): a non-header row's bit set or cleared
/// in the CVar pair, written with `%d` through the CVar store, never Lua's `SetCVar` (`0x5b04a4`); the verb rebuilds the list after (`0x5b09ff`, `0x5b0a6f`).
fn set_mark(lua: &Lua, index: i64, on: bool, pair: &str) {
    let mut model = super::model_mut_of(lua);
    let write = (|| {
        let entry = usize::try_from(index)
            .ok()
            .and_then(|i| model.currency_list.get(i))?;
        if entry.header {
            return None;
        }
        let item = entry.item_id;
        let bit_index = with_catalog(|c| {
            c.types
                .iter()
                .find(|t| t.item_id == item)
                .map(|t| t.bit_index)
        })
        .flatten()?;
        let (name, bit) = match bit_index {
            b @ 1..=32 => (format!("{pair}1"), b - 1),
            b @ 33..=64 => (format!("{pair}2"), b - 33),
            _ => return None,
        };
        let old = cvar_int(&model, &name);
        let new = if on {
            old | (1 << bit)
        } else {
            old & !(1 << bit)
        };
        Some((name, (new as i32).to_string()))
    })();
    if let Some((name, value)) = write {
        super::cvars::set_from_engine(&mut model, &name, value);
    }
    build(&mut model);
}

/// The setters' two numbers, both tested first (`0x5b094a`, `0x5b0959`, `0x5b09c9`), truncated.
fn index_flag_args(
    lua: &Lua,
    index: Value,
    flag: Value,
    usage: &'static str,
) -> mlua::Result<(i64, bool)> {
    match (lua.coerce_number(index)?, lua.coerce_number(flag)?) {
        (Some(i), Some(f)) => Ok((
            i64::from(super::binding_abi::ftol(i)) - 1,
            super::binding_abi::ftol(f) != 0,
        )),
        _ => Err(mlua::Error::runtime(usage)),
    }
}

/// A tooltip method's usage error, filled with the tooltip's own name or `<unnamed>` (`0x630670`).
fn tooltip_usage(lua: &Lua, this: &Table, call: &str) -> mlua::Error {
    let name = super::object::frame_handle_of(lua, this)
        .ok()
        .and_then(|h| read(lua).arena.frame(h).and_then(|f| f.name.clone()));
    mlua::Error::runtime(format!(
        "Usage: {}:{call}",
        name.as_deref()
            .unwrap_or(super::object::anchor_args::UNNAMED)
    ))
}

fn opt_str(lua: &Lua, s: Option<String>) -> mlua::Result<Value> {
    Ok(match s {
        Some(s) => Value::String(lua.create_string(&s)?),
        None => Value::Nil,
    })
}

/// Register the currency globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // GetCurrencyListSize() (`0x5afd10`).
    g.set(
        "GetCurrencyListSize",
        lua.create_function(|lua, _: MultiValue| Ok(read(lua).currency_list.len() as f64))?,
    )?;

    // GetCurrencyListInfo(index) (`0x5b0680`): name, isHeader, isExpanded, isUnused, isWatched,
    // count, extraCurrencyType, icon, itemID; nine nils out of range.
    g.set(
        "GetCurrencyListInfo",
        lua.create_function(|lua, index: Value| {
            let i = index_arg(lua, index, "Usage: GetCurrencyListInfo(index)")?;
            let model = read(lua);
            let Some(e) = usize::try_from(i)
                .ok()
                .and_then(|i| model.currency_list.get(i))
            else {
                return Ok(MultiValue::from_iter(std::iter::repeat_n(Value::Nil, 9)));
            };
            // A header is unused when its category sorts last (`0x5b0518`), a row by its CVar bit, both read at the call (`0x5b071c`).
            let unused = with_catalog(|c| {
                if e.header {
                    sorted_categories(c)
                        .last()
                        .is_some_and(|last| last.id == e.category_id)
                } else {
                    c.types
                        .iter()
                        .find(|t| t.item_id == e.item_id)
                        .is_some_and(|t| cvar_bit(&model, "currencyTokensUnused", t.bit_index))
                }
            })
            .unwrap_or(false);
            let h = holdings();
            let (count, kind) = match (e.header, count_of(h.as_ref(), e.item_id)) {
                (true, _) => (Value::Number(0.0), Value::Integer(0)),
                (false, Some((n, k))) => {
                    (Value::Number(f64::from(n)), Value::Integer(i64::from(k)))
                }
                (false, None) => (Value::Number(0.0), Value::Boolean(false)),
            };
            let icon = if e.header {
                None
            } else {
                icon_of(h.as_ref(), e.item_id)
            };
            Ok(MultiValue::from_iter([
                opt_str(lua, e.name.clone())?,
                Value::Boolean(e.header),
                Value::Boolean(e.expanded),
                Value::Boolean(unused),
                Value::Boolean(e.watched),
                count,
                kind,
                opt_str(lua, icon)?,
                Value::Integer(i64::from(e.item_id)),
            ]))
        })?,
    )?;

    // ExpandCurrencyList(index, expand) (`0x5b0940`): a header's category opens or closes.
    g.set(
        "ExpandCurrencyList",
        lua.create_function(|lua, (index, expand): (Value, Value)| {
            let (i, expand) = index_flag_args(
                lua,
                index,
                expand,
                "Usage: ExpandCurrencyList(index,expand)",
            )?;
            let mut model = super::model_mut_of(lua);
            let header = usize::try_from(i)
                .ok()
                .and_then(|i| model.currency_list.get(i))
                .filter(|e| e.header)
                .map(|e| e.category_id);
            if let Some(id) = header {
                model.currency_collapsed.insert(id, !expand);
            }
            // The rebuild after the setter (`0x5b098f`).
            build(&mut model);
            Ok(())
        })?,
    )?;

    // SetCurrencyUnused(index, unused) (`0x5b09b0`), SetCurrencyBackpack(index, watched)
    // (`0x5b0a20`).
    g.set(
        "SetCurrencyUnused",
        lua.create_function(|lua, (index, on): (Value, Value)| {
            let (i, on) =
                index_flag_args(lua, index, on, "Usage: SetCurrencyUnused(index,unused)")?;
            set_mark(lua, i, on, "currencyTokensUnused");
            Ok(())
        })?,
    )?;
    g.set(
        "SetCurrencyBackpack",
        lua.create_function(|lua, (index, on): (Value, Value)| {
            let (i, on) =
                index_flag_args(lua, index, on, "Usage: SetCurrencyBackpack(index,backpack)")?;
            set_mark(lua, i, on, "currencyTokensBackpack");
            Ok(())
        })?,
    )?;

    // GetBackpackCurrencyInfo(index) (`0x5b0a90`): name, count, extraCurrencyType, icon, itemID
    // of the index-th watched currency; five nils past them.
    g.set(
        "GetBackpackCurrencyInfo",
        lua.create_function(|lua, index: Value| {
            let i = index_arg(lua, index, "Usage: GetBackpackCurrencyInfo(index)")?;
            let model = read(lua);
            let h = holdings();
            let Some(t) = usize::try_from(i)
                .ok()
                .and_then(|i| backpack_currency(&model, h.as_ref(), i))
            else {
                return Ok(MultiValue::from_iter(std::iter::repeat_n(Value::Nil, 5)));
            };
            // An uncached item record answers five nils (`0x5b0b0d`).
            let Some(name) = h
                .as_ref()
                .and_then(|h| h.items.get(&t.item_id))
                .and_then(|(n, _)| n.clone())
            else {
                return Ok(MultiValue::from_iter(std::iter::repeat_n(Value::Nil, 5)));
            };
            let (count, kind) = count_of(h.as_ref(), t.item_id).unwrap_or((0, 0));
            Ok(MultiValue::from_iter([
                Value::String(lua.create_string(&name)?),
                Value::Number(f64::from(count)),
                Value::Number(f64::from(kind)),
                opt_str(lua, icon_of(h.as_ref(), t.item_id))?,
                Value::Integer(i64::from(t.item_id)),
            ]))
        })?,
    )?;
    Ok(())
}

/// `GameTooltip:SetCurrencyToken(index)` (`0x630620`) and `:SetBackpackToken(index)` (`0x6307e0`):
/// the currency's item tooltip, from the held token when one is; nothing for a header, an index
/// past the list or no player (`0x6306db`).
pub(crate) fn install_tooltip_methods(lua: &Lua, m: &Table) -> mlua::Result<()> {
    m.set(
        "SetCurrencyToken",
        lua.create_function(|lua, (this, index): (Table, Value)| {
            let Some(n) = lua.coerce_number(index)? else {
                return Err(tooltip_usage(lua, &this, "SetCurrencyToken(index)"));
            };
            let i = i64::from(super::binding_abi::ftol(n)) - 1;
            let item = {
                let model = read(lua);
                usize::try_from(i)
                    .ok()
                    .and_then(|i| model.currency_list.get(i))
                    .filter(|e| !e.header)
                    .map(|e| e.item_id)
            };
            match item {
                Some(id) => super::tooltip_item::render_token(lua, &this, id, false),
                None => Ok(()),
            }
        })?,
    )?;
    m.set(
        "SetBackpackToken",
        lua.create_function(|lua, (this, index): (Table, Value)| {
            let Some(n) = lua.coerce_number(index)? else {
                return Err(tooltip_usage(lua, &this, "SetBackpackToken(index)"));
            };
            let i = i64::from(super::binding_abi::ftol(n)) - 1;
            let item = {
                let model = read(lua);
                usize::try_from(i)
                    .ok()
                    .and_then(|i| backpack_currency(&model, holdings().as_ref(), i))
                    .map(|t| t.item_id)
            };
            match item {
                Some(id) => super::tooltip_item::render_token(lua, &this, id, true),
                None => Ok(()),
            }
        })?,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::live::{lending, test_view, TestView};
    use crate::script::UiScript;

    fn catalog() -> CurrencyCatalogView {
        CurrencyCatalogView {
            types: vec![
                CurrencyTypeView {
                    item_id: 43308,
                    category_id: 1,
                    bit_index: 1,
                },
                CurrencyTypeView {
                    item_id: 29434,
                    category_id: 2,
                    bit_index: 2,
                },
                CurrencyTypeView {
                    item_id: 20560,
                    category_id: 1,
                    bit_index: 40,
                },
                CurrencyTypeView {
                    item_id: 40752,
                    category_id: 2,
                    bit_index: 5,
                },
            ],
            categories: vec![
                CurrencyCategoryView {
                    id: 3,
                    flags: 3,
                    name: "Unused".into(),
                },
                CurrencyCategoryView {
                    id: 2,
                    flags: 0,
                    name: "Dungeon".into(),
                },
                CurrencyCategoryView {
                    id: 1,
                    flags: 0,
                    name: "Player vs. Player".into(),
                },
            ],
        }
    }

    fn holdings() -> CurrencyHoldings {
        let items = [
            (43308, "Honor Points"),
            (29434, "Badge of Justice"),
            (20560, "Alterac Valley Mark of Honor"),
        ]
        .into_iter()
        .map(|(id, n)| {
            (
                id,
                (
                    Some(n.to_string()),
                    Some(format!("Interface\\Icons\\I{id}")),
                ),
            )
        })
        .collect();
        CurrencyHoldings {
            known: 1 | 2 | (1 << 39) | (1 << 4),
            tokens: vec![(29434, 12), (20560, 3)],
            honor: 1500,
            arena: 0,
            items,
        }
    }

    /// A view answering the tables and `holdings`.
    fn lend(catalog: CurrencyCatalogView, holdings: CurrencyHoldings) -> TestView {
        TestView {
            currency_catalog: Some(catalog),
            currency_holdings: Some(holdings),
            ..Default::default()
        }
    }

    /// The test VM and the view of the tables and our currencies, its list built under it as a trigger builds it.
    fn script() -> (UiScript, TestView) {
        let mut loan = lend(catalog(), holdings());
        let mut s = UiScript::new().unwrap();
        s.register_cvars([
            ("currencyTokensUnused1", "0"),
            ("currencyTokensUnused2", "0"),
            ("currencyTokensBackpack1", "0"),
            ("currencyTokensBackpack2", "0"),
        ]);
        lending(&mut loan, || s.rebuild_currency_list());
        (s, loan)
    }

    fn row(s: &UiScript, i: i64) -> (Option<String>, bool, bool, bool, bool, f64) {
        s.eval(&format!(
            "local n, h, e, u, w, c = GetCurrencyListInfo({i}) return n, h, e, u, w, c"
        ))
        .unwrap()
    }

    /// The list: headers by name with the flagged one last, rows by name with the uncached one
    /// last, the honor count from the field and a token's from its slot.
    #[test]
    fn the_list_groups_known_rows_under_sorted_headers() {
        let (s, mut loan_v) = script();
        crate::script::live::lending(&mut loan_v, || {
            assert_eq!(s.eval::<i64>("return GetCurrencyListSize()").unwrap(), 6);
            assert_eq!(row(&s, 1).0.as_deref(), Some("Dungeon"));
            assert!(row(&s, 1).1, "a header");
            assert_eq!(row(&s, 2).0.as_deref(), Some("Badge of Justice"));
            assert_eq!(row(&s, 2).5, 12.0);
            assert_eq!(row(&s, 3).0, None, "uncached sorts last");
            assert_eq!(row(&s, 4).0.as_deref(), Some("Player vs. Player"));
            assert_eq!(
                row(&s, 5).0.as_deref(),
                Some("Alterac Valley Mark of Honor")
            );
            assert_eq!(row(&s, 5).5, 3.0);
            assert_eq!(row(&s, 6).0.as_deref(), Some("Honor Points"));
            assert_eq!(row(&s, 6).5, 1500.0, "honor reads the field");
            assert_eq!(
                s.eval::<i64>("return select('#', GetCurrencyListInfo(7))")
                    .unwrap(),
                9,
                "nine nils past the list"
            );
            assert!(s
                .eval::<bool>("return GetCurrencyListInfo(7) == nil")
                .unwrap());
            assert!(s.run("GetCurrencyListInfo()").is_err());
        });
    }

    /// The build asks only a listed row's uncached record (`0x5b01ef`); its landing rebuilds once (`0x5b01d7` -> `0x5b0660`), a later landing of it nothing.
    #[test]
    fn the_build_asks_a_listed_rows_record_and_its_landing_rebuilds() {
        let (mut s, mut loan_v) = script();
        crate::script::live::lending(&mut loan_v, || {
            assert_eq!(
                test_view(|v| v.item_asks.borrow().clone()),
                [40752],
                "the uncached Dungeon row"
            );
            test_view(|v| v.item_asks.borrow_mut().clear());
            s.run("ExpandCurrencyList(1, 0)").unwrap();
            assert!(
                test_view(|v| v.item_asks.borrow().is_empty()),
                "a folded row is not listed, so not asked"
            );
            s.run("ExpandCurrencyList(1, 1)").unwrap();
            let mut named = holdings();
            named
                .items
                .insert(40752, (Some("Emblem of Heroism".into()), None));
            let mut named_v = lend(catalog(), named);
            crate::script::live::lending(&mut named_v, || {
                assert_eq!(row(&s, 3).0, None, "the list holds what its build read");
                assert!(s.currency_record_landed(40752), "a build waited on it");
                assert_eq!(row(&s, 2).0.as_deref(), Some("Badge of Justice"));
                assert_eq!(row(&s, 3).0.as_deref(), Some("Emblem of Heroism"));
                assert!(!s.currency_record_landed(40752), "its wait is over");
                assert!(!s.currency_record_landed(2589), "never asked");
            });
        });
    }

    /// A count reads our currencies at the call, the list's names at its build.
    #[test]
    fn a_count_reads_the_call_and_a_name_the_build() {
        let (s, mut loan_v) = script();
        crate::script::live::lending(&mut loan_v, || {
            let mut more = holdings();
            more.tokens = vec![(29434, 40)];
            more.items.insert(29434, (Some("Renamed".into()), None));
            let mut more_v = lend(catalog(), more);
            crate::script::live::lending(&mut more_v, || {
                assert_eq!(row(&s, 2).5, 40.0, "the count is live");
                assert_eq!(
                    row(&s, 2).0.as_deref(),
                    Some("Badge of Justice"),
                    "the name is the build's"
                );
            });
        });
    }

    /// Collapsing hides a header's rows; marking a row unused moves it under the last category
    /// and writes the CVar bit; a watched row reaches the backpack verbs.
    #[test]
    fn collapse_unused_and_backpack_marks() {
        let (s, mut loan_v) = script();
        crate::script::live::lending(&mut loan_v, || {
            s.run("ExpandCurrencyList(1, 0)").unwrap();
            assert_eq!(s.eval::<i64>("return GetCurrencyListSize()").unwrap(), 4);
            assert!(!row(&s, 1).2, "collapsed");
            s.run("ExpandCurrencyList(1, 1)").unwrap();
            // The badge (bit 2) moves under the last category, whose header now shows.
            s.run("SetCurrencyUnused(2, 1)").unwrap();
            assert_eq!(
                s.eval::<String>("return GetCVar('currencyTokensUnused1')")
                    .unwrap(),
                "2"
            );
            assert_eq!(s.eval::<i64>("return GetCurrencyListSize()").unwrap(), 7);
            assert_eq!(row(&s, 6).0.as_deref(), Some("Unused"));
            assert_eq!(row(&s, 7).0.as_deref(), Some("Badge of Justice"));
            assert!(row(&s, 7).3, "unused");
            // Row 4 is now the Alterac Valley mark (bit 40, the second CVar's bit 7).
            s.run("SetCurrencyBackpack(4, 1)").unwrap();
            let (name, count, kind, _icon, id): (String, f64, f64, Option<String>, i64) =
                s.eval("return GetBackpackCurrencyInfo(1)").unwrap();
            assert_eq!(
                (name.as_str(), count, kind, id),
                ("Alterac Valley Mark of Honor", 3.0, 0.0, 20560)
            );
            assert_eq!(
                s.eval::<String>("return GetCVar('currencyTokensBackpack2')")
                    .unwrap(),
                "128"
            );
            assert!(s
                .eval::<bool>("return GetBackpackCurrencyInfo(2) == nil")
                .unwrap());
        });
    }

    /// The three setters take two numbers or raise their usage, and a flag truncates: `true`, nil
    /// and a word raise, `0.5` is off, a numeric string passes.
    #[test]
    fn the_setters_take_two_numbers_and_truncate_the_flag() {
        let (s, mut loan_v) = script();
        crate::script::live::lending(&mut loan_v, || {
            for (call, usage) in [
                (
                    "ExpandCurrencyList",
                    "Usage: ExpandCurrencyList(index,expand)",
                ),
                (
                    "SetCurrencyUnused",
                    "Usage: SetCurrencyUnused(index,unused)",
                ),
                (
                    "SetCurrencyBackpack",
                    "Usage: SetCurrencyBackpack(index,backpack)",
                ),
            ] {
                for args in ["1, true", "1, nil", "1", "1, 'on'", "nil, 1", "'x', 1"] {
                    let err = s.run(&format!("{call}({args})")).unwrap_err().to_string();
                    assert!(err.contains(usage), "{call}({args}): {err}");
                }
            }
            s.run("ExpandCurrencyList(1, 0.5)").unwrap();
            assert!(!row(&s, 1).2, "0.5 truncates to 0: collapsed");
            s.run("ExpandCurrencyList('1', '1')").unwrap();
            assert!(row(&s, 1).2, "numeric strings pass");
            s.run("SetCurrencyUnused(2, 0.9)").unwrap();
            assert_eq!(
                s.eval::<String>("return GetCVar('currencyTokensUnused1')")
                    .unwrap(),
                "0"
            );
            s.run("SetCurrencyUnused(2.7, -1)").unwrap();
            assert_eq!(
                s.eval::<String>("return GetCVar('currencyTokensUnused1')")
                    .unwrap(),
                "2",
                "2.7 truncates to row 2, -1 is on"
            );
        });
    }

    /// A Badge of Justice template, class 10 and bind on pickup, and a tooltip `TT` whose lines and
    /// money [`token_tip`] reads.
    fn with_token_tip(s: &mut UiScript) {
        s.set_item_template(
            29434,
            crate::script::ItemTemplateView {
                name: "Badge of Justice".into(),
                class: 10,
                bonding: 1,
                sell_price: 5,
                ..Default::default()
            },
        );
        s.run(
            r#"
            ITEM_BIND_ON_PICKUP = "[BOP]"
            local a = CreateFrame("Button", "Slot")
            CreateFrame("GameTooltip", "TT"):SetOwner(Slot, "ANCHOR_RIGHT")
            TT:SetScript("OnTooltipAddMoney", function(_, cost) Money = cost end)
        "#,
        )
        .unwrap();
    }

    /// `call` on `TT`: its lines joined by `|`, and the money it added.
    fn token_tip(s: &UiScript, call: &str) -> (String, Option<f64>) {
        s.run(&format!("Money = nil TT:{call}")).unwrap();
        s.eval(
            r#"local t = {}
            for i = 1, TT:NumLines() do t[i] = _G["TTTextLeft" .. i]:GetText() or "" end
            return table.concat(t, "|"), Money"#,
        )
        .unwrap()
    }

    /// A held token builds from its slot's object with the player as requester (`0x630789`): no
    /// bind line, and only `SetBackpackToken` prices the stack (`0x63097d`).
    #[test]
    fn a_held_token_builds_from_its_slot_without_the_bind_line() {
        let (mut s, mut loan_v) = script();
        crate::script::live::lending(&mut loan_v, || {
            with_token_tip(&mut s);
            s.run("SetCurrencyBackpack(2, 1)").unwrap();
            let mut slots = crate::script::ContainerState {
                num_slots: 32,
                ..Default::default()
            };
            slots.slots.insert(
                4,
                crate::script::ContainerSlot {
                    guid: 0x4000_0000_0000_0099,
                    item_id: 29434,
                    count: 12,
                    link: Some(
                        "|cffffffff|Hitem:29434:0:0:0:0:0:0:0:80|h[Badge of Justice]|h|r".into(),
                    ),
                    ..Default::default()
                },
            );
            s.set_token_slots(Some(slots));
            assert_eq!(
                token_tip(&s, "SetCurrencyToken(2)"),
                ("Badge of Justice".into(), None)
            );
            assert_eq!(
                token_tip(&s, "SetBackpackToken(1)"),
                ("Badge of Justice".into(), Some(60.0)),
                "the held stack of 12 at 5 each"
            );
            // With loot open the player is still not its source.
            s.set_loot_source(0xF130_0000_0000_0001);
            assert_eq!(token_tip(&s, "SetCurrencyToken(2)").0, "Badge of Justice");
        });
    }

    /// An unheld token builds by id with a zero requester (`0x630750`), which equals the loot
    /// source `[0xbfa8d8]` only while no loot is open (`0x627efd`); neither binding prices it.
    #[test]
    fn an_unheld_token_shows_the_bind_line_only_with_no_loot_open() {
        let (mut s, mut loan_v) = script();
        crate::script::live::lending(&mut loan_v, || {
            with_token_tip(&mut s);
            s.run("SetCurrencyBackpack(2, 1)").unwrap();
            s.set_token_slots(Some(crate::script::ContainerState {
                num_slots: 32,
                ..Default::default()
            }));
            for call in ["SetCurrencyToken(2)", "SetBackpackToken(1)"] {
                assert_eq!(
                    token_tip(&s, call),
                    ("Badge of Justice|[BOP]".into(), None),
                    "{call}"
                );
            }
            s.set_loot_source(0xF130_0000_0000_0001);
            assert_eq!(
                token_tip(&s, "SetCurrencyToken(2)").0,
                "Badge of Justice",
                "a zero requester is not an open loot's source"
            );
        });
    }

    /// The tooltip methods' usage names the tooltip they were called on (`0xa26b00`, `0xa26b24`).
    #[test]
    fn a_tooltip_usage_names_its_own_frame() {
        let (s, mut loan_v) = script();
        crate::script::live::lending(&mut loan_v, || {
            s.run(
                r#"CreateFrame("GameTooltip", "CurrencyTip") Unnamed = CreateFrame("GameTooltip")"#,
            )
            .unwrap();
            for (call, want) in [
                (
                    "CurrencyTip:SetCurrencyToken()",
                    "Usage: CurrencyTip:SetCurrencyToken(index)",
                ),
                (
                    "CurrencyTip:SetBackpackToken('x')",
                    "Usage: CurrencyTip:SetBackpackToken(index)",
                ),
                (
                    "Unnamed:SetCurrencyToken(nil)",
                    "Usage: <unnamed>:SetCurrencyToken(index)",
                ),
            ] {
                let err = s.run(call).unwrap_err().to_string();
                assert!(err.contains(want), "{call}: {err}");
            }
            s.run("CurrencyTip:SetCurrencyToken(99) CurrencyTip:SetBackpackToken(99)")
                .unwrap();
        });
    }

    /// Marking a row writes the CVar pair without calling Lua's `SetCVar`, hooked or replaced.
    #[test]
    fn a_mark_writes_the_cvar_without_the_lua_setcvar() {
        let (s, mut loan_v) = script();
        crate::script::live::lending(&mut loan_v, || {
            s.run("Calls = 0 hooksecurefunc('SetCVar', function() Calls = Calls + 1 end)")
                .unwrap();
            s.run("SetCurrencyBackpack(2, 1) SetCurrencyUnused(2, 1)")
                .unwrap();
            assert_eq!(
                s.eval::<String>("return GetCVar('currencyTokensUnused1')")
                    .unwrap(),
                "2"
            );
            // The badge now sits last, under the unused header.
            s.run("SetCVar = function() error('replaced') end SetCurrencyUnused(7, 0)")
                .unwrap();
            assert_eq!(s.eval::<i64>("return Calls").unwrap(), 0);
            let (unused, watched): (String, String) = s
                .eval("return GetCVar('currencyTokensUnused1'), GetCVar('currencyTokensBackpack1')")
                .unwrap();
            assert_eq!((unused.as_str(), watched.as_str()), ("0", "2"));
        });
    }

    /// A header reports unused only for the category that sorts last.
    #[test]
    fn only_the_last_category_header_is_unused() {
        let (s, mut loan_v) = script();
        crate::script::live::lending(&mut loan_v, || {
            s.run("SetCurrencyUnused(2, 1)").unwrap();
            let headers: Vec<(String, bool)> = (1..=7)
                .map(|i| row(&s, i))
                .filter(|r| r.1)
                .map(|r| (r.0.unwrap(), r.3))
                .collect();
            assert_eq!(
                headers,
                [
                    ("Dungeon".to_string(), false),
                    ("Player vs. Player".to_string(), false),
                    ("Unused".to_string(), true),
                ]
            );
        });
    }

    /// A watched token whose item record has not arrived answers five nils.
    #[test]
    fn an_uncached_watched_token_answers_nothing() {
        let (s, mut loan_v) = script();
        crate::script::live::lending(&mut loan_v, || {
            // Row 3 is the uncached token (bit 5).
            s.run("SetCurrencyBackpack(3, 1)").unwrap();
            assert_eq!(
                s.eval::<String>("return GetCVar('currencyTokensBackpack1')")
                    .unwrap(),
                "16"
            );
            let (n, all_nil): (i64, bool) = s
            .eval(
                "local a, b, c, d, e = GetBackpackCurrencyInfo(1)                  return select('#', GetBackpackCurrencyInfo(1)),                  a == nil and b == nil and c == nil and d == nil and e == nil",
            )
            .unwrap();
            assert_eq!((n, all_nil), (5, true));
        });
    }

    /// Categories and the rows within one sort with accents folded: `É` sorts as `E`, before `Z`.
    #[test]
    fn categories_and_rows_fold_accents_when_sorting() {
        let (mut s, mut loan_v) = script();
        crate::script::live::lending(&mut loan_v, || {
            let mut catalog = catalog();
            catalog.categories[1].name = "Zone".into();
            catalog.categories[2].name = "Événements".into();
            let mut holdings = holdings();
            holdings.items.get_mut(&43308).unwrap().0 = Some("Points".into());
            holdings.items.get_mut(&20560).unwrap().0 = Some("Écusson".into());
            let mut renamed_v = lend(catalog, holdings);
            crate::script::live::lending(&mut renamed_v, || {
                s.rebuild_currency_list();
                let names: Vec<Option<String>> = (1..=6).map(|i| row(&s, i).0).collect();
                let want = [
                    "Événements",
                    "Écusson",
                    "Points",
                    "Zone",
                    "Badge of Justice",
                ];
                assert_eq!(names[..5], want.map(|n| Some(n.to_string())), "{names:?}");
            });
        });
    }

    /// A `ReloadUI` carries the closed categories into the new VM; a fresh VM, as a world entry from
    /// the character screen builds, has every category open.
    #[test]
    fn closed_categories_survive_a_reload_and_open_on_a_new_vm() {
        let (mut s, mut loan_v) = script();
        crate::script::live::lending(&mut loan_v, || {
            s.run("ExpandCurrencyList(1, 0)").unwrap();
            let carried = s.take_currency_folds();
            let (mut fresh, mut fresh_loan_v) = script();
            crate::script::live::lending(&mut fresh_loan_v, || {
                assert!(row(&fresh, 1).2, "a new VM starts open");
                fresh.restore_currency_folds(carried);
                assert!(!row(&fresh, 1).2, "the reload kept it closed");
                assert_eq!(
                    fresh.eval::<i64>("return GetCurrencyListSize()").unwrap(),
                    4
                );
            });
        });
    }
}
