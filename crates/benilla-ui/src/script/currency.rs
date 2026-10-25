//! 3.3.5a's currency list (`CGCurrencyTypes`): `CurrencyCategory.dbc`'s headers, each over its
//! known `CurrencyTypes.dbc` rows, the rows marked unused gathered under the category that sorts
//! last, and the backpack's watched tokens. The unused and watched marks are the per-character
//! `currencyTokens*` CVars, one bit per currency's `BitIndex` (`0x5b0440`, `0x5b0590`).

use mlua::{Lua, MultiValue, Table, Value};

/// Honor and arena points, counted from the player's fields, not a token slot (`0x5b0757`).
const ARENA_POINTS_ITEM: u32 = 43307;
const HONOR_POINTS_ITEM: u32 = 43308;
/// The display list's capacity (`0xc1e960`, 95 entries of 24 bytes).
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

/// One display-list entry.
#[derive(Clone, Debug, PartialEq)]
struct Entry {
    name: Option<String>,
    item_id: u32,
    category_id: u32,
    header: bool,
    expanded: bool,
    watched: bool,
}

impl super::UiScript {
    /// Push both currency tables.
    pub fn set_currency_catalog(&mut self, catalog: CurrencyCatalogView) {
        set_field!(self, currency_catalog, catalog);
    }

    /// Push the player's currency holdings, `None` with no player.
    pub fn set_currency_holdings(&mut self, holdings: Option<CurrencyHoldings>) {
        set_field!(self, currency_holdings, holdings);
    }
}

fn read(lua: &Lua) -> mlua::AppDataRef<'_, super::ModelCell> {
    lua.app_data_ref::<super::ModelCell>()
        .expect("model app_data")
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

fn known(model: &super::Model, bit: i32) -> bool {
    let known = model.currency_holdings.as_ref().map_or(0, |h| h.known);
    (1..=64).contains(&bit) && known & (1 << (bit - 1)) != 0
}

/// The categories in display order (`0x5afc40`): unflagged first, then by name, any case.
fn sorted_categories(model: &super::Model) -> Vec<&CurrencyCategoryView> {
    let mut cats: Vec<&CurrencyCategoryView> = model.currency_catalog.categories.iter().collect();
    cats.sort_by(|a, b| {
        (a.flags & 1)
            .cmp(&(b.flags & 1))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    cats
}

/// `BuildDisplayList` (`0x5b0030`): a header per category with a row to show, then, expanded,
/// its known rows not marked unused, or under the last category every row marked unused; each
/// category's rows by name, uncached names last.
fn display_list(model: &super::Model) -> Vec<Entry> {
    let cats = sorted_categories(model);
    let types = &model.currency_catalog.types;
    let name_of = |item: u32| {
        model
            .currency_holdings
            .as_ref()
            .and_then(|h| h.items.get(&item))
            .and_then(|(n, _)| n.clone())
    };
    let mut out = Vec::new();
    for (i, cat) in cats.iter().enumerate() {
        let last = i + 1 == cats.len();
        // `CategoryHasKnownTypes` (`0x5afe30`): a known row of the category, or for the last one
        // any row marked unused, known or not.
        let has_known = types.iter().any(|t| {
            (t.category_id == cat.id && known(model, t.bit_index))
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
            .filter(|t| known(model, t.bit_index))
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
        rows.sort_by(|a, b| match (&a.name, &b.name) {
            (Some(x), Some(y)) => x.to_lowercase().cmp(&y.to_lowercase()),
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
fn count_of(model: &super::Model, item_id: u32) -> Option<(u32, u32)> {
    let h = model.currency_holdings.as_ref()?;
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

fn icon_of(model: &super::Model, item_id: u32) -> Option<String> {
    model
        .currency_holdings
        .as_ref()
        .and_then(|h| h.items.get(&item_id))
        .and_then(|(_, icon)| icon.clone())
}

/// The `i`-th currency (0-based) in file order both known and watched (`0x5b0340`).
fn backpack_currency(model: &super::Model, i: usize) -> Option<CurrencyTypeView> {
    model
        .currency_catalog
        .types
        .iter()
        .filter(|t| {
            known(model, t.bit_index) && cvar_bit(model, "currencyTokensBackpack", t.bit_index)
        })
        .nth(i)
        .copied()
}

/// A 1-based list index, `lua_isnumber` gated (`0x5b0680`), made 0-based.
fn index_arg(lua: &Lua, v: Value, usage: &'static str) -> mlua::Result<i64> {
    match lua.coerce_number(v)? {
        Some(n) => Ok(super::binding_abi::ftol(n) as i64 - 1),
        None => Err(mlua::Error::runtime(usage)),
    }
}

/// `SetUnused`/`SetBackpack` (`0x5b0440`, `0x5b0590`): a non-header row's bit set or cleared in the
/// CVar pair, written back with `%d`.
fn set_mark(lua: &Lua, index: i64, on: bool, pair: &str) {
    let write = {
        let model = read(lua);
        let list = display_list(&model);
        let Some(entry) = usize::try_from(index).ok().and_then(|i| list.get(i)) else {
            return;
        };
        if entry.header {
            return;
        }
        let Some(t) = model
            .currency_catalog
            .types
            .iter()
            .find(|t| t.item_id == entry.item_id)
        else {
            return;
        };
        let (name, bit) = match t.bit_index {
            b @ 1..=32 => (format!("{pair}1"), b - 1),
            b @ 33..=64 => (format!("{pair}2"), b - 33),
            _ => return,
        };
        let old = cvar_int(&model, &name);
        let new = if on {
            old | (1 << bit)
        } else {
            old & !(1 << bit)
        };
        (name, (new as i32).to_string())
    };
    let _ = lua
        .globals()
        .get::<mlua::Function>("SetCVar")
        .and_then(|f| f.call::<()>((write.0, write.1)));
}

/// A flag argument: `lua_toboolean` for a boolean, nonzero for a number (`0x5b09d5`).
fn flag_arg(v: &Value) -> bool {
    match v {
        Value::Nil | Value::Boolean(false) => false,
        Value::Integer(i) => *i != 0,
        Value::Number(n) => *n != 0.0,
        _ => true,
    }
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
        lua.create_function(|lua, _: MultiValue| Ok(display_list(&read(lua)).len() as f64))?,
    )?;

    // GetCurrencyListInfo(index) (`0x5b0680`): name, isHeader, isExpanded, isUnused, isWatched,
    // count, extraCurrencyType, icon, itemID; nine nils out of range.
    g.set(
        "GetCurrencyListInfo",
        lua.create_function(|lua, index: Value| {
            let i = index_arg(lua, index, "Usage: GetCurrencyListInfo(index)")?;
            let model = read(lua);
            let list = display_list(&model);
            let Some(e) = usize::try_from(i).ok().and_then(|i| list.get(i)) else {
                return Ok(MultiValue::from_iter(std::iter::repeat_n(Value::Nil, 9)));
            };
            let unused = !e.header
                && model
                    .currency_catalog
                    .types
                    .iter()
                    .find(|t| t.item_id == e.item_id)
                    .is_some_and(|t| cvar_bit(&model, "currencyTokensUnused", t.bit_index));
            let (count, kind) = match (e.header, count_of(&model, e.item_id)) {
                (true, _) => (Value::Number(0.0), Value::Integer(0)),
                (false, Some((n, k))) => {
                    (Value::Number(f64::from(n)), Value::Integer(i64::from(k)))
                }
                (false, None) => (Value::Number(0.0), Value::Boolean(false)),
            };
            let icon = if e.header {
                None
            } else {
                icon_of(&model, e.item_id)
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
            let i = index_arg(lua, index, "Usage: ExpandCurrencyList(index, expand)")?;
            let mut model = super::model_mut_of(lua);
            let list = display_list(&model);
            if let Some(e) = usize::try_from(i).ok().and_then(|i| list.get(i)) {
                if e.header {
                    let id = e.category_id;
                    model.currency_collapsed.insert(id, !flag_arg(&expand));
                }
            }
            Ok(())
        })?,
    )?;

    // SetCurrencyUnused(index, unused) (`0x5b09b0`), SetCurrencyBackpack(index, watched)
    // (`0x5b0a20`).
    g.set(
        "SetCurrencyUnused",
        lua.create_function(|lua, (index, on): (Value, Value)| {
            let i = index_arg(lua, index, "Usage: SetCurrencyUnused(index, unused)")?;
            set_mark(lua, i, flag_arg(&on), "currencyTokensUnused");
            Ok(())
        })?,
    )?;
    g.set(
        "SetCurrencyBackpack",
        lua.create_function(|lua, (index, on): (Value, Value)| {
            let i = index_arg(lua, index, "Usage: SetCurrencyBackpack(index, backpack)")?;
            set_mark(lua, i, flag_arg(&on), "currencyTokensBackpack");
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
            let Some(t) = usize::try_from(i)
                .ok()
                .and_then(|i| backpack_currency(&model, i))
            else {
                return Ok(MultiValue::from_iter(std::iter::repeat_n(Value::Nil, 5)));
            };
            let name = model
                .currency_holdings
                .as_ref()
                .and_then(|h| h.items.get(&t.item_id))
                .and_then(|(n, _)| n.clone());
            let (count, kind) = count_of(&model, t.item_id).unwrap_or((0, 0));
            Ok(MultiValue::from_iter([
                opt_str(lua, name)?,
                Value::Number(f64::from(count)),
                Value::Number(f64::from(kind)),
                opt_str(lua, icon_of(&model, t.item_id))?,
                Value::Integer(i64::from(t.item_id)),
            ]))
        })?,
    )?;
    Ok(())
}

/// `GameTooltip:SetCurrencyToken(index)` (`0x630620`) and `:SetBackpackToken(index)` (`0x6307e0`):
/// the currency's item tooltip; nothing for a header or an index past the list.
pub(crate) fn install_tooltip_methods(lua: &Lua, m: &Table) -> mlua::Result<()> {
    m.set(
        "SetCurrencyToken",
        lua.create_function(|lua, (this, index): (Table, Value)| {
            let i = index_arg(lua, index, "Usage: GameTooltip:SetCurrencyToken(index)")?;
            let item = {
                let model = read(lua);
                usize::try_from(i)
                    .ok()
                    .and_then(|i| display_list(&model).get(i).cloned())
                    .filter(|e| !e.header)
                    .map(|e| e.item_id)
            };
            match item {
                Some(id) => super::tooltip_item::render_by_id(lua, &this, id, Default::default()),
                None => Ok(()),
            }
        })?,
    )?;
    m.set(
        "SetBackpackToken",
        lua.create_function(|lua, (this, index): (Table, Value)| {
            let i = index_arg(lua, index, "Usage: GameTooltip:SetBackpackToken(index)")?;
            let item = {
                let model = read(lua);
                usize::try_from(i)
                    .ok()
                    .and_then(|i| backpack_currency(&model, i))
                    .map(|t| t.item_id)
            };
            match item {
                Some(id) => super::tooltip_item::render_by_id(lua, &this, id, Default::default()),
                None => Ok(()),
            }
        })?,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::UiScript;

    fn script() -> UiScript {
        let mut s = UiScript::new().unwrap();
        s.register_cvars([
            ("currencyTokensUnused1", "0"),
            ("currencyTokensUnused2", "0"),
            ("currencyTokensBackpack1", "0"),
            ("currencyTokensBackpack2", "0"),
        ]);
        s.set_currency_catalog(CurrencyCatalogView {
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
        });
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
        s.set_currency_holdings(Some(CurrencyHoldings {
            known: 1 | 2 | (1 << 39) | (1 << 4),
            tokens: vec![(29434, 12), (20560, 3)],
            honor: 1500,
            arena: 0,
            items,
        }));
        s
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
        let s = script();
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
    }

    /// Collapsing hides a header's rows; marking a row unused moves it under the last category
    /// and writes the CVar bit; a watched row reaches the backpack verbs.
    #[test]
    fn collapse_unused_and_backpack_marks() {
        let s = script();
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
        s.run("SetCurrencyBackpack(4, true)").unwrap();
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
    }
}
