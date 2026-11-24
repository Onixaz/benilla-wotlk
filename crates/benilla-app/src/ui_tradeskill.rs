//! The crafting book: the client's tradeskill statics ([`TradeSkillOpen`], `0xc235a0`-`0xc23604`)
//! around `benilla_ui::script`'s `tradeskill` list. There is no wire: a profession opener
//! (`SPELL_EFFECT_TRADE_SKILL` in `Effect[0]`) never reaches the send, as the cast attempt
//! (`0x80d0ab`) opens the window client-side through the trade skill's open (`0x5de120`), a step
//! of the press that cast it.
//!
//! The book lists each known spell with `SPELL_ATTR_IS_TRADESKILL` (`0x20`, which also hides it
//! from the spellbook) whose `SkillLineAbility` row joins the open line. The list rebuild
//! (`0x5dd5a0`) runs where 12340 calls it, each a step of its handler: the open, a known recipe
//! added or removed (`0x6e7c1a`, `0x6e72e6`), the skill watches (`0x6da5fa`, `0x6cddb7`), the slot,
//! bag and count watches (`0x5d96bc`, `0x5d9859`, `0x5d9c05`, `0x7077db`), an item's post-init and
//! disable (`0x70ade4`, `0x70b0c4`), our visible items (`0x6e1e9c`) and the item callbacks
//! (`0x5d9f94`, `0x5de10b`). The verbs read the list and the game at the call.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_formats::{SpellDisplay, SPELL_ATTR_IS_TRADESKILL};
use benilla_protocol::messages::ItemInfo;
use benilla_ui::script::{
    TradeSkillDifficulty, TradeSkillGroup, TradeSkillLineView, TradeSkillList,
    TradeSkillReagentView, TradeSkillRecipeView, TradeSkillRow,
};
use benilla_ui::strings::Arg;

use crate::creature_anim::{CastEvent, CastEventKind};
use crate::entities::ItemDisplays;
use crate::items::{Items, RecordWait, RecordWaits};
use crate::net::{GuidIndex, NetCommands, ObjectStore, Objects, SelfGuid, SelfPlayer};
use crate::spell::{cast_target, CastCommit, CastLadder};
use crate::ui_action::{PlayerActions, Spells};
use crate::ui_items::{count_of, item_icon, tool_categories, InventoryScope};
use crate::ui_script::live::UiEvents;
use crate::ui_spellbook::SkillLines;
use benilla_assets::{AssetSet, LockRecover, WorldAssets};

/// `SPELL_EFFECT_CREATE_ITEM`, `_CREATE_RANDOM_ITEM` and `_CREATE_ITEM_2`: the effects a recipe
/// makes its product with (`0x5dd5a0`).
const CREATE_ITEM_EFFECTS: [u32; 3] = [24, 59, 157];

/// Inventory type to slot-filter bits (`0xa2d288`).
pub(crate) const INVENTORY_TYPE_SLOTS: [u32; 29] = [
    0, 1, 2, 4, 8, 16, 32, 64, 128, 256, 512, 3072, 12288, 98304, 65536, 131072, 16384, 98304,
    7864320, 262144, 16, 98304, 98304, 65536, 0, 131072, 131072, 0, 131072,
];

/// The slot menu's catch-all bit (`NONEQUIPSLOT`).
const SLOT_NONEQUIP: u32 = 0x400_0000;

/// The item-stat tokens by stat type, the name filter's (`0x5dc110`).
const ITEM_MOD_TOKENS: [&str; 49] = [
    "ITEM_MOD_MANA",
    "ITEM_MOD_HEALTH",
    "",
    "ITEM_MOD_AGILITY",
    "ITEM_MOD_STRENGTH",
    "ITEM_MOD_INTELLECT",
    "ITEM_MOD_SPIRIT",
    "ITEM_MOD_STAMINA",
    "",
    "",
    "",
    "",
    "ITEM_MOD_DEFENSE_SKILL_RATING",
    "ITEM_MOD_DODGE_RATING",
    "ITEM_MOD_PARRY_RATING",
    "ITEM_MOD_BLOCK_RATING",
    "ITEM_MOD_HIT_MELEE_RATING",
    "ITEM_MOD_HIT_RANGED_RATING",
    "ITEM_MOD_HIT_SPELL_RATING",
    "ITEM_MOD_CRIT_MELEE_RATING",
    "ITEM_MOD_CRIT_RANGED_RATING",
    "ITEM_MOD_CRIT_SPELL_RATING",
    "ITEM_MOD_HIT_TAKEN_MELEE_RATING",
    "ITEM_MOD_HIT_TAKEN_RANGED_RATING",
    "ITEM_MOD_HIT_TAKEN_SPELL_RATING",
    "ITEM_MOD_CRIT_TAKEN_MELEE_RATING",
    "ITEM_MOD_CRIT_TAKEN_RANGED_RATING",
    "ITEM_MOD_CRIT_TAKEN_SPELL_RATING",
    "ITEM_MOD_HASTE_MELEE_RATING",
    "ITEM_MOD_HASTE_RANGED_RATING",
    "ITEM_MOD_HASTE_SPELL_RATING",
    "ITEM_MOD_HIT_RATING",
    "ITEM_MOD_CRIT_RATING",
    "ITEM_MOD_HIT_TAKEN_RATING",
    "ITEM_MOD_CRIT_TAKEN_RATING",
    "ITEM_MOD_RESILIENCE_RATING",
    "ITEM_MOD_HASTE_RATING",
    "ITEM_MOD_EXPERTISE_RATING",
    "ITEM_MOD_ATTACK_POWER",
    "ITEM_MOD_RANGED_ATTACK_POWER",
    "",
    "ITEM_MOD_SPELL_HEALING_DONE",
    "ITEM_MOD_SPELL_DAMAGE_DONE",
    "ITEM_MOD_MANA_REGENERATION",
    "ITEM_MOD_ARMOR_PENETRATION_RATING",
    "ITEM_MOD_SPELL_POWER",
    "ITEM_MOD_HEALTH_REGEN",
    "ITEM_MOD_SPELL_PENETRATION",
    "ITEM_MOD_BLOCK_VALUE",
];

/// The difficulty band on the raw rank (`0x5dd5a0`), a zero `trivialLow` read as
/// `trivialHigh - 25`, or 0 when trivialHigh is 25.
pub(crate) fn difficulty(rank: u32, low: u32, high: u32) -> TradeSkillDifficulty {
    let (rank, high) = (i64::from(rank), i64::from(high));
    let low = match i64::from(low) {
        0 if high != 25 => high - 25,
        0 => 0,
        l => l,
    };
    if rank < low {
        TradeSkillDifficulty::Optimal
    } else if rank < (high + low) / 2 {
        TradeSkillDifficulty::Medium
    } else if rank < high {
        TradeSkillDifficulty::Easy
    } else {
        TradeSkillDifficulty::Trivial
    }
}

/// The slot-filter bits of a product's `InventoryType`: a bag, a finger and a trinket take one
/// bit, a type with none the catch-all (`0x5dd5a0`).
fn product_slots(inventory_type: u32) -> u32 {
    let mask = match inventory_type {
        18 => 0x8_0000,
        11 => 0x400,
        12 => 0x1000,
        t => INVENTORY_TYPE_SLOTS.get(t as usize).copied().unwrap_or(0),
    };
    if mask == 0 {
        SLOT_NONEQUIP
    } else {
        mask
    }
}

/// The slot-filter bits of a recipe without a product, off the spell's equipped-item
/// requirement: a one-hand or two-hand weapon enchant, the first armor slot, or a shield
/// (`0x5dd5a0`).
fn enchant_slots(d: &SpellDisplay) -> u32 {
    match d.equipped_item_class {
        2 if d.equipped_item_subclass_mask & 1 != 0 => 0x80_0000,
        2 => 0x100_0000,
        4 if d.equipped_item_inventory_type_mask != 0 => {
            let first = (0..29).find(|j| d.equipped_item_inventory_type_mask & (1 << j) != 0);
            let mask = first.map_or(0, |j| {
                let m = INVENTORY_TYPE_SLOTS[j];
                if m & 0x8_0000 != 0 {
                    m & 0xff8f_ffff
                } else if m & 0x400 != 0 {
                    m & !0x800
                } else if m & 0x1000 != 0 {
                    m & !0x2000
                } else {
                    m
                }
            });
            if mask == 0 {
                SLOT_NONEQUIP
            } else {
                mask
            }
        }
        4 if d.equipped_item_subclass_mask & 0x40 != 0 => 0x200_0000,
        _ => SLOT_NONEQUIP,
    }
}

/// `GetTradeSkillNumMade` (`0x5db6e0`): effect 0's points, `BasePoints + 1` to
/// `BasePoints + DieSides`, each at least 1. Deviation: no per-level or spell-mod term, which no
/// recipe carries.
fn num_made(d: &SpellDisplay) -> (u32, u32) {
    let base = d.effect_base_points[0];
    let min = base.saturating_add(1).max(1) as u32;
    let max = base.saturating_add(d.effect_die_sides[0]).max(1) as u32;
    (min, max)
}

/// A stat or resistance line as the name filter reads it, the template's `%c` taking the sign.
/// Deviation: the reference passes the sign to a `%d`-only rating template too (`0x5dc110`),
/// which prints its character code; the value is printed instead.
fn signed_line(template: &str, value: i32, rest: &[Arg<'_>]) -> String {
    let sign = if value > 0 { "+" } else { "-" };
    let mut args = Vec::with_capacity(2 + rest.len());
    if template.contains("%c") {
        args.push(Arg::S(sign));
    }
    args.push(Arg::D(i64::from(value).abs()));
    args.extend_from_slice(rest);
    benilla_ui::strings::fill(template, &args)
}

/// The product's own texts the name filter searches, in the reference's filter order (`0x5dc110`):
/// its stats, resistances, spell lines and description. Sockets and a gem's enchantment name are
/// not read: the template keeps neither.
fn product_texts(
    t: &ItemInfo,
    global: &dyn Fn(&str) -> Option<String>,
    spell_text: &dyn Fn(u32) -> Option<String>,
    out: &mut Vec<String>,
) {
    for &(stat, value) in &t.stats {
        let Some(token) = ITEM_MOD_TOKENS.get(stat as usize).filter(|k| !k.is_empty()) else {
            continue;
        };
        if value <= 0 {
            continue;
        }
        if let Some(template) = global(token) {
            out.push(signed_line(&template, value, &[]));
        }
    }
    // Field i is school i + 1; Holy only decides whether the schools differ.
    let all = t.resistances[0];
    if t.resistances[1..].iter().any(|&r| r != all) {
        for (i, &r) in t.resistances.iter().enumerate().skip(1) {
            if r == 0 {
                continue;
            }
            let school = global(&format!("SPELL_SCHOOL{}_CAP", i + 1)).unwrap_or_default();
            if let Some(template) = global("ITEM_RESIST_SINGLE") {
                out.push(signed_line(&template, r, &[Arg::S(&school)]));
            }
        }
    } else if all != 0 {
        if let Some(template) = global("ITEM_RESIST_ALL") {
            out.push(signed_line(&template, all, &[]));
        }
    }
    for s in &t.spells {
        let Some(text) = spell_text(s.spell_id) else {
            continue;
        };
        let prefix = match s.trigger {
            0 => global("ITEM_SPELL_TRIGGER_ONUSE"),
            1 => global("ITEM_SPELL_TRIGGER_ONEQUIP"),
            2 => global("ITEM_SPELL_TRIGGER_ONPROC"),
            _ => None,
        };
        out.push(match prefix {
            Some(p) => format!("{p} {text}"),
            None => text,
        });
    }
    if !t.description.is_empty() {
        out.push(t.description.clone());
    }
}

/// The client's tradeskill statics (`0xc235a0`-`0xc23604`), which the verbs read and write at the
/// call and the rebuild writes at its sites.
#[derive(Resource, Default)]
pub(crate) struct TradeSkillOpen {
    pub(crate) list: TradeSkillList,
}

impl TradeSkillOpen {
    /// The open line, `None` with none (`[0xc235a4]`).
    pub(crate) fn line(&self) -> Option<u32> {
        (self.list.line != 0).then_some(self.list.line)
    }
}

/// `SpellFocusObject.dbc`, the names on a "Requires: Anvil" line.
#[derive(Resource)]
pub(crate) struct SpellFocus {
    pub(crate) catalog: benilla_formats::SpellFocusCatalog,
}

pub(crate) struct UiTradeSkillPlugin;

impl Plugin for UiTradeSkillPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TradeSkillOpen>()
            .init_resource::<TradeSkillRecipes>()
            .add_systems(Startup, load_spell_focus.after(AssetSet::Open))
            // After the UI shutdown's close and `PLAYER_LOGOUT` (`0x529160`: `0x528f00` at
            // `0x529163`, then `0x5dcaf0` at `0x5291a4`).
            .add_systems(
                OnExit(crate::char_select::ClientState::InWorld),
                shutdown_game.after(crate::ui_script::end_ui_session),
            )
            .add_systems(
                Update,
                // In the press pass after the clicks, so a recast's GCD reaches the frame's tick (`0x480410` before `0x495320`).
                drain_trade_skill
                    .in_set(crate::ui_script::UiPress)
                    .after(crate::script_calls::apply_script_calls),
            );
    }
}

/// The game shutdown's trade-skill reset (`0x5dcaf0`), which keeps the filters it does not write.
fn shutdown_game(mut open: ResMut<TradeSkillOpen>) {
    open.list.shutdown();
}

/// Our player's recipe arrays, one per line, the hash the rebuild finds the open line's in
/// (`player+0x185c`, `0x6e73b0`): a node is made by the first recipe of its line added
/// (`0x6e7c07`) and stays when its removals empty it (`0x6e72df`); our object's init starts empty.
#[derive(Resource, Default)]
pub(crate) struct TradeSkillRecipes(std::collections::BTreeMap<u32, Vec<u32>>);

impl TradeSkillRecipes {
    /// `line`'s array in add order (`0x6e73b0`); `None` with no node, the rebuild's exit
    /// (`0x5dd7b6`).
    fn line(&self, line: u32) -> Option<&[u32]> {
        self.0.get(&line).map(Vec::as_slice)
    }
}

/// Our player's store by the active player's guid through the object lookup (`0x4d3790`,
/// `0x4d4db0`), found from our own create's init on, before the self tag lands.
fn own_store(world: &World) -> Option<&ObjectStore> {
    let guid = world.get_resource::<SelfGuid>()?.0?;
    let entity = *world.get_resource::<GuidIndex>()?.0.get(&guid)?;
    world.get::<ObjectStore>(entity)
}

/// The line a recipe files under for our race and class (`0x6e7b90`, `0x6e7250`); `None` for a
/// spell without `SPELL_ATTR_IS_TRADESKILL` (`0x6e7bdb`, `0x6e7230`), without an ability row, or
/// whose row names a line `SkillLine.dbc` lacks (`0x6e7b9f`-`0x6e7bbe`, `0x6e7264`-`0x6e7270`).
fn recipe_line(world: &mut World, spell_id: u32) -> Option<u32> {
    let d = world.get_resource::<Spells>()?.catalog.get(spell_id)?;
    if d.attributes & SPELL_ATTR_IS_TRADESKILL == 0 {
        return None;
    }
    let (race, class) = crate::spell::race_class(own_store(world));
    let catalog = &world.get_resource::<SkillLines>()?.catalog;
    let line = catalog.ability(spell_id, race, class)?.skill_id;
    catalog.line(line).map(|_| line)
}

/// A known spell's add to our player (`0x6e7b00`): a recipe joins its line's array, the node made
/// at the first (`0x6e7c07`, `0x6e7c11`), then the list rebuilds whole (`0x6e7c1a`), with a line
/// or without.
pub(crate) fn recipe_learned(world: &mut World, spell_id: u32) {
    let is_recipe = world
        .get_resource::<Spells>()
        .and_then(|s| s.catalog.get(spell_id))
        .is_some_and(|d| d.attributes & SPELL_ATTR_IS_TRADESKILL != 0);
    if !is_recipe {
        return;
    }
    if let Some(line) = recipe_line(world, spell_id) {
        if let Some(mut recipes) = world.get_resource_mut::<TradeSkillRecipes>() {
            recipes.0.entry(line).or_default().push(spell_id);
        }
    }
    refresh(world, true);
}

/// A known spell's removal from our player (`0x6e71d0`): a recipe in its line's array leaves it,
/// the node kept (`0x6e72df`), and the list rebuilds whole (`0x6e72e6`); one in no array does not.
pub(crate) fn recipe_removed(world: &mut World, spell_id: u32) {
    let Some(line) = recipe_line(world, spell_id) else {
        return;
    };
    let removed = world
        .get_resource_mut::<TradeSkillRecipes>()
        .and_then(|mut r| {
            let list = r.0.get_mut(&line)?;
            // The array is searched from its tail (`0x6e72a5`).
            let at = list.iter().rposition(|&s| s == spell_id)?;
            list.remove(at);
            Some(())
        })
        .is_some();
    if removed {
        refresh(world, true);
    }
}

/// Our object's init (`0x6e7f50`): a fresh recipe hash ahead of its adds, each of which files its
/// recipe through `recipe_learned` (`0x6e7fc5`).
pub(crate) fn clear_recipes(world: &mut World) {
    if let Some(mut recipes) = world.get_resource_mut::<TradeSkillRecipes>() {
        recipes.0.clear();
    }
}

fn load_spell_focus(mut commands: Commands, assets: Option<Res<WorldAssets>>) {
    let Some(assets) = assets else { return };
    let loaded = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_spell_focus_catalog(&mut chain)
    };
    match loaded {
        Ok(catalog) => {
            debug!("ui_tradeskill: {} spell-focus name(s)", catalog.len());
            commands.insert_resource(SpellFocus { catalog });
        }
        Err(e) => warn!(
            "ui_tradeskill: SpellFocusObject.dbc failed to load — the Requires line drops the \
             focus name: {e:#}"
        ),
    }
}

/// A spell's line for `guid`'s name record (`0x5da5f0`): the record looked up without an ask, a
/// miss answering none (`0x5da61d`, `0x5da624`); the ability for the record's race and class
/// (`0x5da629`-`0x5da638`); its line's `SkillLine.dbc` row, none without one (`0x5da647`-`0x5da661`).
fn line_rec(
    names: &crate::names::NameCache,
    guid: u64,
    store: Option<&ObjectStore>,
    catalog: &benilla_formats::SkillLineCatalog,
    spell_id: u32,
) -> Option<u32> {
    names.peek(guid)?;
    // Our own name is seeded at world entry with no traits filed; our descriptor carries the pair.
    let (race, class) = names
        .player_traits(guid)
        .map_or_else(|| crate::spell::race_class(store), |(r, c, _)| (r, c));
    let line = catalog.ability(spell_id, race, class)?.skill_id;
    catalog.line(line).map(|_| line)
}

/// The opener's line (`0x5de131` -> `0x5da5f0`) for its caster, our player: benilla takes the
/// opener branch (`0x80d08c`) for our own press alone.
fn opener_line(world: &World, spell_id: u32) -> Option<u32> {
    let caster = world.get_resource::<SelfGuid>()?.0?;
    let names = world.get_resource::<crate::names::NameCache>()?;
    let catalog = &world.get_resource::<SkillLines>()?.catalog;
    line_rec(names, caster, own_store(world), catalog, spell_id)
}

/// The open (`0x5de120`) for opener `spell_id`, a step of the press whose cast attempt took the
/// opener branch (`0x80d0ab`): the opener's line (`0x5de131` -> `0x5da5f0`), else nothing at all
/// (`0x5de13f`); a running repeat stops and signals (`0x5de1c4`), then the rest at its own step.
pub(crate) fn set_trade_skill(world: &mut World, spell_id: u32) {
    let Some(line) = opener_line(world, spell_id) else {
        debug!("ui_tradeskill: opener {spell_id} has no line for our name record — no open");
        return;
    };
    let stopped = world
        .get_resource_mut::<TradeSkillOpen>()
        .is_some_and(|mut o| o.list.stop_recast());
    if stopped {
        if let Some(mut ui) = world.get_resource_mut::<UiEvents>() {
            ui.fire("UPDATE_TRADESKILL_RECAST", Vec::new());
        }
    }
    crate::ui_script::live::in_turn(world, move |world| open_rest(world, spell_id, line));
}

/// The open after its repeat stop: the item callbacks cleared (`0x5de1cc`); the opener of the line
/// already open closes it (`0x5de1eb`); else the line switch and the rebuild with its update
/// (`0x5de2c3`), then the show at its own step.
fn open_rest(world: &mut World, spell_id: u32, line: u32) {
    clear_item_callbacks(world);
    let Some(mut open) = world.get_resource_mut::<TradeSkillOpen>() else {
        return;
    };
    if open.list.line == line {
        debug!("ui_tradeskill: opener {spell_id} closes skill line {line}");
        close(world);
        return;
    }
    debug!("ui_tradeskill: opener {spell_id} opens skill line {line}");
    open.list.open_line(line, spell_id);
    refresh(world, true);
    crate::ui_script::live::in_turn(world, open_show);
}

/// The open's tail, after the rebuild's update ran its listeners: the line as it now stands kept as
/// the last (`0x5de2c8`, `0x5de2d7`), and `TRADE_SKILL_SHOW` with any entry (`0x5de2d1`,
/// `0x5de2e5`).
fn open_show(world: &mut World) {
    let Some(mut open) = world.get_resource_mut::<TradeSkillOpen>() else {
        return;
    };
    open.list.opened();
    if open.list.entry_count() != 0 {
        if let Some(mut ui) = world.get_resource_mut::<UiEvents>() {
            ui.fire("TRADE_SKILL_SHOW", Vec::new());
        }
    }
}

/// The close (`0x5da5a0`): no line or opener, a running repeat stopped and signalled (`0x5da5d1`),
/// then `TRADE_SKILL_CLOSE` (`0x5da5df`).
pub(crate) fn close(world: &mut World) {
    let stopped = world.resource_mut::<TradeSkillOpen>().list.close();
    if let Some(mut ui) = world.get_resource_mut::<UiEvents>() {
        if stopped {
            ui.fire("UPDATE_TRADESKILL_RECAST", Vec::new());
        }
        ui.fire("TRADE_SKILL_CLOSE", Vec::new());
    }
}

/// The UI shutdown's close (`0x528f96` -> `0x5da5a0`), after the trade cancel and before the world
/// leave, on the reload's root (`0x52b03e`) and the game shutdown's (`0x529163`): its events reach
/// the dying VM.
pub(crate) fn shutdown_close(world: &mut World) {
    if world.get_resource::<TradeSkillOpen>().is_none() {
        return;
    }
    close(world);
    crate::ui_script::live::fire_handler_events(world);
}

/// The world leave's part (`0x528d0f` -> `0x5dcae0`): the item callbacks cleared.
pub(crate) fn leave_world(world: &mut World) {
    clear_item_callbacks(world);
}

/// The item callbacks the rebuild left, cleared (`0x5dad10`): none waits and none is pending.
fn clear_item_callbacks(world: &mut World) {
    let pending = world
        .get_resource::<TradeSkillOpen>()
        .is_some_and(|o| o.list.items_pending != 0);
    if !pending {
        return;
    }
    if let Some(mut waits) = world.get_non_send_resource_mut::<RecordWaits>() {
        waits.cancel(|w| w == RecordWait::TradeSkillList);
    }
    world.resource_mut::<TradeSkillOpen>().list.items_pending = 0;
}

/// The list rebuild (`0x5dd5a0`), with `full` the sub-classes too: the item callbacks cleared,
/// each known recipe of the open line filed against the rank and the bags as they stand, a product
/// record in flight asked with the list's callback (`0x5d9f80`); with none in flight,
/// `TRADE_SKILL_UPDATE` (`0x5de0d5`). Nothing with no line, player or recipe array for the line.
pub(crate) fn refresh(world: &mut World, full: bool) {
    if world.get_resource::<TradeSkillOpen>().is_none() {
        return;
    }
    clear_item_callbacks(world);
    // The name filter's stat lines read the VM's global strings (`0x5dc110`), lent with the `World`.
    let lua = world
        .get_non_send_resource::<benilla_ui::script::UiScript>()
        .map(|s| s.lua().clone());
    let ran = world.run_system_cached_with(refresh_system, (full, lua));
    if let Err(e) = ran {
        warn!("ui_tradeskill: the list rebuild did not run: {e}");
    }
}

fn refresh_system(
    In((full, lua)): In<(bool, Option<mlua::Lua>)>,
    mut open: ResMut<TradeSkillOpen>,
    recipes: Res<TradeSkillRecipes>,
    live: TradeSkillLive,
    actions: Option<Res<PlayerActions>>,
    mut waits: Option<NonSendMut<RecordWaits>>,
    mut ui: Option<ResMut<UiEvents>>,
) {
    let empty = std::collections::BTreeSet::new();
    let known = actions.as_deref().map_or(&empty, |a| &a.spells);
    let line = open.list.line;
    let mut pending = Vec::new();
    // The open line's array (`0x5dd7ac`); no player, or no array, exits with nothing filed
    // (`0x5dd79e`, `0x5dd7b6`).
    let rows = recipes
        .line(line)
        .filter(|_| line != 0)
        .and_then(|array| live.rows(line, array, &mut pending));
    for &product in &pending {
        if let Some(w) = waits.as_deref_mut() {
            w.wait(product, RecordWait::TradeSkillList, false);
        }
    }
    open.list.items_pending += pending.len() as u32;
    let (name, min, max) = {
        let (n, a, b) = open.list.text_filter();
        (n.map(str::to_string), a, b)
    };
    let global = |key: &str| {
        lua.as_ref()
            .and_then(|lua| benilla_ui::strings::global(lua, key))
    };
    let passes = |spell: u32| live.filter_spell(spell, min, max, name.as_deref(), known, &global);
    let fired = open.list.refresh(full, rows, &passes);
    debug!(
        "ui_tradeskill: rebuild (full {full}) of line {line}: {} entr(ies), {} record(s) in flight",
        open.list.entry_count(),
        open.list.items_pending
    );
    if fired {
        if let Some(ui) = ui.as_deref_mut() {
            ui.fire("TRADE_SKILL_UPDATE", Vec::new());
        }
    }
}

/// The list's product callback (`0x5d9f80`): one fewer in flight, and the last rebuilds the list
/// whole (`0x5d9f94`).
pub(crate) fn list_record_callback(world: &mut World) {
    let Some(mut open) = world.get_resource_mut::<TradeSkillOpen>() else {
        return;
    };
    if open.list.items_pending == 0 {
        return;
    }
    open.list.items_pending -= 1;
    if open.list.items_pending == 0 {
        refresh(world, true);
    }
}

/// A reagent's or tool's callback (`0x5de100`): a found record rebuilds the list in place
/// (`0x5de10b`).
pub(crate) fn item_record_callback(world: &mut World, landed: bool) {
    if landed {
        refresh(world, false);
    }
}

/// The list rebuild in place, a step of the watch that calls it.
pub(crate) fn refresh_in_place(world: &mut World) {
    refresh(world, false);
}

/// A line's raw rank half (`0x5dd8cf`-`0x5dd8e5`); a missing line reads slot -1, PLAYER_XP ([`crate::ui_char::found_skill_slot`]).
fn raw_rank(store: &ObjectStore, skill_id: u32) -> u32 {
    u32::from(crate::ui_char::found_skill_slot(&store.0, skill_id).value)
}

/// What the rebuild and the verbs read of the game at the call: the spell, skill, item and tool
/// catalogs, the item cache and the player's bags.
#[derive(SystemParam)]
pub(crate) struct TradeSkillLive<'w, 's> {
    spells: Option<Res<'w, Spells>>,
    spell_mods: Option<Res<'w, crate::spell::SpellModifiers>>,
    skill_lines: Option<Res<'w, SkillLines>>,
    focus: Option<Res<'w, SpellFocus>>,
    area_groups: Option<Res<'w, crate::area::AreaGroupsRes>>,
    areas: Option<Res<'w, crate::area::AreaTableRes>>,
    icons: Option<Res<'w, ItemDisplays>>,
    subclasses: Option<Res<'w, crate::ui_items::ItemSubClasses>>,
    totem_categories: Option<Res<'w, crate::ui_totem::TotemCategories>>,
    items: Option<Res<'w, Items>>,
    commands: Option<Res<'w, NetCommands>>,
    self_guid: Option<Res<'w, SelfGuid>>,
    names: Option<Res<'w, crate::names::NameCache>>,
    /// Absent in an app without the net plugin, whose bags hold nothing.
    objects: Option<Objects<'w, 's>>,
    index: Option<Res<'w, GuidIndex>>,
    stores: Query<
        'w,
        's,
        (
            &'static ObjectStore,
            Option<&'static crate::unit_auras::UnitAuras>,
        ),
    >,
}

impl TradeSkillLive<'_, '_> {
    /// Our player as the rebuild finds it, by the active player's guid through the object lookup
    /// (`0x5dd656` -> `0x4d3790`, `0x5dd669` -> `0x4d4db0`), so our own create's init finds it.
    fn me(&self) -> Option<(&ObjectStore, Option<&crate::unit_auras::UnitAuras>)> {
        let guid = self.self_guid.as_deref()?.0?;
        let entity = *self.index.as_deref()?.0.get(&guid)?;
        self.stores.get(entity).ok()
    }

    fn store(&self) -> Option<&ObjectStore> {
        self.me().map(|(store, _)| store)
    }

    /// How many of `entry` the bags hold (`0x754d00`).
    fn have(&self, store: &ObjectStore, entry: u32) -> u32 {
        self.objects
            .as_ref()
            .map_or(0, |o| count_of(&store.0, o, entry, InventoryScope::DEFAULT))
    }

    /// An item record from the cache without an ask.
    fn cached(&self, entry: u32) -> Option<&ItemInfo> {
        self.items.as_deref()?.template_cached(entry)
    }

    /// An item record from the cache, a miss asked for (`0x67ca30`).
    fn asked(&self, entry: u32) -> Option<&ItemInfo> {
        let (items, commands) = (self.items.as_deref()?, self.commands.as_deref()?);
        items.template(entry, 0, commands)
    }

    /// The open line's recipes as the rebuild files them (`0x5dd5a0`), entry i of the line's
    /// array in add order (`0x5dd910`); a create-item recipe whose product record misses joins
    /// `pending`, asked (`0x5ddaf1`). `None` with no player.
    fn rows(&self, line: u32, array: &[u32], pending: &mut Vec<u32>) -> Option<Vec<TradeSkillRow>> {
        // The list's owner is our guid, as benilla opens no linked list (#1021).
        let owner = self.self_guid.as_deref()?.0?;
        let names = self.names.as_deref()?;
        // The owner's name record, looked up without an ask; a miss exits (`0x5dd696`, `0x5dd6a0`).
        names.peek(owner)?;
        let store = self.store()?;
        let spells = self.spells.as_deref()?;
        let skill_lines = self.skill_lines.as_deref()?;
        let rank = raw_rank(store, line);
        // The ability lookup's race and class are the record's (`0x5dd916`, `0x5dd91c`), else our descriptor's.
        let (race, class) = names
            .player_traits(owner)
            .map_or_else(|| crate::spell::race_class(Some(store)), |(r, c, _)| (r, c));
        let mut rows = Vec::new();
        for &spell_id in array {
            let Some(d) = spells.catalog.get(spell_id) else {
                continue;
            };
            // The recipe's ability for our race and class (`0x5dd928` -> `0x812410`).
            let Some(sla) = skill_lines.catalog.ability(spell_id, race, class) else {
                continue;
            };
            // The fewest crafts the bags cover (`0x5dda7c` -> `0x754d00`), -1 with no reagent.
            let mut num_available: i32 = -1;
            for &(entry, need) in d.reagents.iter() {
                if entry == 0 || need == 0 || num_available == 0 {
                    continue;
                }
                let have = self.have(store, entry);
                let can = (have / need) as i32;
                if num_available == -1 || num_available >= can {
                    num_available = can;
                }
            }
            let creates_item = CREATE_ITEM_EFFECTS.contains(&d.effects[0]);
            let product_item = d.effect_item_type[0];
            let (group, item_level, inv_slots) = if creates_item && product_item != 0 {
                match self.asked(product_item) {
                    Some(t) => (
                        Some(TradeSkillGroup::SubClass {
                            class: t.class,
                            subclass: t.subclass,
                            name: self
                                .subclasses
                                .as_deref()
                                .and_then(|sc| sc.0.name(t.class, t.subclass))
                                .map(str::to_string),
                        }),
                        t.item_level,
                        product_slots(t.inventory_type),
                    ),
                    None => {
                        pending.push(product_item);
                        (None, 0, SLOT_NONEQUIP)
                    }
                }
            } else {
                (Some(TradeSkillGroup::Verb), 0, enchant_slots(d))
            };
            rows.push(TradeSkillRow {
                spell_id,
                name: d.name.clone(),
                difficulty: difficulty(rank, sla.trivial_low, sla.trivial_high),
                num_available,
                trivial_high: sla.trivial_high,
                group,
                item_level,
                inv_slots,
            });
        }
        Some(rows)
    }

    /// The open line at the call (`0x5db810`): `SkillLine.dbc`'s name and alternate verb, the
    /// player's value read (`0x51a250`) and max read (`0x594710`).
    pub(crate) fn line_view(&self, line: u32) -> TradeSkillLineView {
        let Some(l) = self
            .skill_lines
            .as_deref()
            .and_then(|s| s.catalog.line(line))
            .filter(|_| line != 0)
        else {
            return TradeSkillLineView::default();
        };
        let (rank, max_rank) = self
            .store()
            .map_or((0, 0), |s| crate::ui_char::link_ranks(&s.0, line));
        TradeSkillLineView {
            name: Some(l.name.clone()),
            alt_verb: Some(l.alternate_verb.clone()),
            rank: i64::from(rank),
            max_rank: i64::from(max_rank),
        }
    }

    /// Recipe `spell_id`'s spell record at the call.
    pub(crate) fn recipe(&self, spell_id: u32) -> Option<TradeSkillRecipeView> {
        let d = self.spells.as_deref()?.catalog.get(spell_id)?;
        let creates_item = CREATE_ITEM_EFFECTS.contains(&d.effects[0]);
        let product_item = d.effect_item_type[0];
        // The product's `Item.dbc` display, else the spell's icon (`0x5db604`, `0x5db687`).
        let icon = if creates_item && product_item != 0 {
            self.icons.as_deref().and_then(|i| {
                let row = i.item_dbc.get(&product_item)?;
                Some(i.icon(row.display))
            })
        } else {
            d.icon.clone()
        };
        let (min_made, max_made) = num_made(d);
        // The line for the active player's name record (`0x5db9b6`, `0x5db9c8` -> `0x5da5f0`).
        let line_name = self.link_line(spell_id).and_then(|line| {
            let catalog = &self.skill_lines.as_deref()?.catalog;
            Some(catalog.line(line)?.name.clone())
        });
        Some(TradeSkillRecipeView {
            name: d.name.clone(),
            tradeskill: d.attributes & SPELL_ATTR_IS_TRADESKILL != 0,
            creates_item,
            product_item,
            icon,
            min_made,
            max_made,
            reagents: d
                .reagents
                .iter()
                .copied()
                .filter(|&(e, _)| e != 0)
                .collect(),
            line_name,
        })
    }

    /// Reagent `item` at the call (`0x5de400`): its record asked with the list's item callback
    /// (`0x5de50d` -> `0x5de100`), and the bags' count (`0x754d00`).
    pub(crate) fn reagent(
        &self,
        item: u32,
        waits: Option<&mut RecordWaits>,
    ) -> TradeSkillReagentView {
        let (name, icon) = match self.asked(item) {
            Some(t) => (
                Some(t.name.clone()),
                item_icon(self.icons.as_deref(), t.display_info_id),
            ),
            None => {
                if let Some(w) = waits {
                    w.wait(item, RecordWait::TradeSkillItem, false);
                }
                (None, None)
            }
        };
        let have = self.store().map_or(0, |s| self.have(s, item));
        TradeSkillReagentView { name, icon, have }
    }

    /// The requirements at the call (`0x5de620`): the focus and the group's first area, both met,
    /// then each tool asked with the list's item callback (`0x5de7a5`) and held in the bags, then
    /// each tool category carried; nothing with no player.
    pub(crate) fn tools(
        &self,
        spell_id: u32,
        mut waits: Option<&mut RecordWaits>,
    ) -> Vec<(String, bool)> {
        let mut out = Vec::new();
        let (Some(store), Some(d)) = (
            self.store(),
            self.spells.as_deref().and_then(|s| s.catalog.get(spell_id)),
        ) else {
            return out;
        };
        if d.requires_spell_focus != 0 {
            if let Some(name) = self
                .focus
                .as_deref()
                .and_then(|f| f.catalog.name(d.requires_spell_focus))
            {
                out.push((name.to_string(), true));
            }
        }
        if d.required_areas_id > 0 {
            let area = (|| {
                let group = self
                    .area_groups
                    .as_deref()?
                    .0
                    .get(d.required_areas_id as u32)?;
                self.areas.as_deref()?.0.name(group.areas[0])
            })();
            if let Some(area) = area {
                out.push((area.to_string(), true));
            }
        }
        for &t in d.totems.iter().filter(|&&t| t != 0) {
            match self.asked(t) {
                Some(info) => {
                    let have = self.have(store, t) > 0;
                    out.push((info.name.clone(), have));
                }
                None => {
                    if let Some(w) = waits.as_deref_mut() {
                        w.wait(t, RecordWait::TradeSkillItem, false);
                    }
                }
            }
        }
        if let Some(cats) = self.totem_categories.as_deref() {
            let wanted: Vec<u32> = d
                .totem_categories
                .iter()
                .copied()
                .filter(|&c| c != 0)
                .collect();
            if !wanted.is_empty() {
                let carried = match (
                    self.items.as_deref(),
                    self.commands.as_deref(),
                    self.objects.as_ref(),
                ) {
                    (Some(items), Some(commands), Some(objects)) => {
                        tool_categories(&store.0, objects, items, commands, InventoryScope::DEFAULT)
                    }
                    _ => Vec::new(),
                };
                for c in wanted {
                    if let Some(row) = cats.0.get(c) {
                        let have = cats.0.covered_by(c, carried.iter().copied());
                        out.push((row.name.clone(), have));
                    }
                }
            }
        }
        out
    }

    /// The spell's description parsed at the call (`0x5dbe10` -> its parser).
    pub(crate) fn description(
        &self,
        spell_id: u32,
        known: &std::collections::BTreeSet<u32>,
        global: &dyn Fn(&str) -> Option<String>,
    ) -> Option<String> {
        let spells = self.spells.as_deref()?;
        let d = spells.catalog.get(spell_id)?;
        let text = d.description.as_deref().filter(|t| !t.is_empty())?;
        let store = self.store()?;
        let items = self.items.as_deref()?;
        let skill_catalog = self.skill_lines.as_deref().map(|s| &*s.catalog);
        let skill = |id| crate::spell::spell_skill_value(Some(store), skill_catalog, id);
        let operands = crate::spell::text_caster::Operands::of(Some(store), items);
        let fallback = crate::spell::SpellModifiers::default();
        let mods = self.spell_mods.as_deref().unwrap_or(&fallback);
        let cost = |d: &SpellDisplay| crate::spell::usable::power_cost(d, store, mods) as i32;
        let caster = crate::spell::text_caster::untracked(
            store,
            (known, self.me().and_then(|(_, auras)| auras)),
            &operands,
            mods,
            &skill,
            &cost,
        );
        let ctx = benilla_formats::TokenContext {
            durations: &spells.durations,
            radii: &spells.radii,
            ranges: Some(&spells.ranges),
            variables: Some(spells.catalog.description_variables()),
            skill: &skill,
            lookup: &|id| spells.catalog.get(spells.for_difficulty(id)),
            mods: Some(mods as &dyn benilla_formats::SpellMods),
            unmodified_points: false,
            gender: &|| store.0.unit_gender().unwrap_or(0),
            home_area: &|| None,
            caster: Some(&caster),
            global,
            printf: &crate::ui_script::token_printf,
        };
        Some(benilla_formats::substitute(text, d, &ctx)).filter(|t| !t.is_empty())
    }

    /// The text filter (`0x5dc110`) on recipe `spell_id` at the call: the product's required level
    /// within `min`..`max` (0 open), then the folded `name` found in the product's texts, the
    /// recipe's name, its reagents', tools', tool categories' and focus' names, or its description.
    pub(crate) fn filter_spell(
        &self,
        spell_id: u32,
        min: i32,
        max: i32,
        name: Option<&str>,
        known: &std::collections::BTreeSet<u32>,
        global: &dyn Fn(&str) -> Option<String>,
    ) -> bool {
        let Some(d) = self.spells.as_deref().and_then(|s| s.catalog.get(spell_id)) else {
            return false;
        };
        let product = self.cached(d.effect_item_type[0]);
        if let Some(t) = product {
            let req = t.required_level as i32;
            if (min != 0 && req < min) || (max != 0 && req > max) {
                return false;
            }
        }
        let Some(name) = name else {
            return true;
        };
        // The text filter (`0x5dc110`) folds each text for case and accents before its search (`0x5dc24f` -> `0x7e11d0`): a lowercase fold for enUS.
        let hit = |text: &str| text.to_lowercase().contains(name);
        let spell_text = |id: u32| self.description(id, known, global);
        if let Some(t) = product {
            let mut texts = Vec::new();
            product_texts(t, global, &spell_text, &mut texts);
            if texts.iter().any(|s| hit(s)) {
                return true;
            }
        }
        if hit(&d.name) {
            return true;
        }
        for &(entry, need) in d.reagents.iter() {
            if entry == 0 || need == 0 {
                continue;
            }
            if self.cached(entry).is_some_and(|t| hit(&t.name)) {
                return true;
            }
        }
        for &t in d.totems.iter().filter(|&&t| t != 0) {
            if self.cached(t).is_some_and(|t| hit(&t.name)) {
                return true;
            }
        }
        if let Some(cats) = self.totem_categories.as_deref() {
            for &c in d.totem_categories.iter().filter(|&&c| c != 0) {
                if cats.0.get(c).is_some_and(|row| hit(&row.name)) {
                    return true;
                }
            }
        }
        if d.requires_spell_focus != 0 {
            if let Some(f) = self
                .focus
                .as_deref()
                .and_then(|f| f.catalog.name(d.requires_spell_focus))
            {
                if hit(f) {
                    return true;
                }
            }
        }
        spell_text(spell_id).is_some_and(|t| hit(&t))
    }

    /// `spell_id`'s line for our name record, a trade link's (`0x5db0df` -> `0x5da5f0`).
    pub(crate) fn link_line(&self, spell_id: u32) -> Option<u32> {
        let me = self.self_guid.as_deref()?.0?;
        let catalog = &self.skill_lines.as_deref()?.catalog;
        line_rec(self.names.as_deref()?, me, self.store(), catalog, spell_id)
    }

    /// The open opener's list link (`0x5dc0b0` -> `0x5db060`), its line the opener's for our name
    /// record (`0x5db0cc`, `0x5db0df` -> `0x5da5f0`), none without one (`0x5db0eb`).
    pub(crate) fn list_link(
        &self,
        opener: u32,
        known: &std::collections::BTreeSet<u32>,
    ) -> Option<String> {
        let store = self.store()?;
        let skill_lines = self.skill_lines.as_deref()?;
        let line = self.link_line(opener)?;
        let opener = self.spells.as_deref()?.catalog.get(opener)?;
        let who = (
            store.0.unit_race().unwrap_or(0),
            store.0.unit_class().unwrap_or(0),
            self.self_guid.as_deref().and_then(|g| g.0).unwrap_or(0),
        );
        let ranks = crate::ui_char::link_ranks(&store.0, line);
        list_link(opener, line, &skill_lines.catalog, known, who, ranks)
    }
}

/// `GetTradeSkillListLink` (`0x5db060`): the opener's `|Htrade:` link, one bit per row of the
/// line's link run, set for a known spell the player's race and class may learn.
pub(crate) fn list_link(
    opener: &SpellDisplay,
    line: u32,
    skill_lines: &benilla_formats::SkillLineCatalog,
    known: &std::collections::BTreeSet<u32>,
    (race, class, guid): (u8, u8, u64),
    (rank, max_rank): (u32, u32),
) -> Option<String> {
    const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    if opener.effects[0] != benilla_formats::SPELL_EFFECT_TRADE_SKILL
        || !skill_lines.line(line).is_some_and(|l| l.can_link)
    {
        return None;
    }
    let run = skill_lines.link_run(line)?;
    let mut bits = String::with_capacity(run.len().div_ceil(6));
    for chunk in run.chunks(6) {
        let mut v = 0usize;
        for (i, a) in chunk.iter().enumerate() {
            if known.contains(&a.spell_id) && a.admits(race, class) {
                v |= 1 << i;
            }
        }
        bits.push(BASE64[v] as char);
    }
    Some(format!(
        "|cffffd000|Htrade:{}:{rank}:{max_rank}:{guid:X}:{bits}|h[{}]|h|r",
        opener.id, opener.name
    ))
}

/// Run the repeat on our cast edges (start, go, failure), each recast its own press so the next reads the GCD it armed (`0x80b583`); the edges land a pass late (#1462).
pub(crate) fn drain_trade_skill(world: &mut World) {
    let edges = world.run_system_cached(own_cast_edges).unwrap_or_default();
    for (kind, spell_id) in edges {
        // The edges write the repeat at once, in the order read (start `0x5da0f0`, go `0x5da070`,
        // failure `0x5da020`): the frame's packets precede its input and paint (`0x47f10d`).
        let Some(mut open) = world.get_resource_mut::<TradeSkillOpen>() else {
            return;
        };
        match kind {
            CastEventKind::Start { .. } => open.list.commit_pending_recast(spell_id),
            CastEventKind::Go { .. } => {
                let (again, fire) = open.list.do_recast(spell_id);
                // The recast, then its signal (`0x5da09f`, `0x5da0ab`).
                if again {
                    debug!("ui_tradeskill: recast {spell_id}");
                    press_trade_skill(world, spell_id);
                }
                if fire {
                    signal_recast(world, 1);
                }
            }
            CastEventKind::Fail { .. } => {
                let fired = open.list.cast_failed(spell_id);
                signal_recast(world, fired);
            }
            _ => {}
        }
    }
}

/// `UPDATE_TRADESKILL_RECAST`, `times` over, fired at this step with the `World` lent.
fn signal_recast(world: &mut World, times: usize) {
    if times == 0 {
        return;
    }
    if let Some(mut ui) = world.get_resource_mut::<UiEvents>() {
        for _ in 0..times {
            ui.fire("UPDATE_TRADESKILL_RECAST", Vec::new());
        }
    }
    crate::ui_script::live::fire_handler_events(world);
}

/// Our own cast edges since the last drain, in the order read.
fn own_cast_edges(
    mut cast_events: MessageReader<CastEvent>,
    me: Query<Entity, With<SelfPlayer>>,
) -> Vec<(CastEventKind, u32)> {
    let me = me.single().ok();
    cast_events
        .read()
        .filter(|ev| Some(ev.entity) == me)
        .map(|ev| (ev.kind, ev.spell_id))
        .collect()
}

/// `DoTradeSkill`'s casts, each a press inside its call (`0x5dacb1` -> `0x80da40`); one that went sets its repeat and signals it (`0x5dacbb`, `0x5dacc1` -> `0x5d9fa0`), one refused sets nothing.
pub(crate) fn press_trade_skill_casts(world: &mut World) {
    let casts = world
        .get_resource_mut::<TradeSkillOpen>()
        .map(|mut o| std::mem::take(&mut o.list.casts))
        .unwrap_or_default();
    for (spell_id, times) in casts {
        debug!("ui_tradeskill: DoTradeSkill({spell_id})");
        if press_trade_skill(world, spell_id) {
            world
                .resource_mut::<TradeSkillOpen>()
                .list
                .set_recast(spell_id, times);
            signal_recast(world, 1);
        }
    }
}

/// One cast as a press, returning the attempt's own verdict: it sent or armed the cursor (`0x80d9b0`, `0x80da1a`), else false (`0x80da2c`).
fn press_trade_skill(world: &mut World, spell_id: u32) -> bool {
    let Some(count) = world
        .get_resource::<crate::spell::PendingCast>()
        .map(|p| p.next_count())
    else {
        return false;
    };
    crate::script_calls::press_step(world, send_trade_skill, spell_id);
    world.resource::<crate::spell::PendingCast>().went(count)
}

fn send_trade_skill(
    In(spell_id): In<u32>,
    targeting: cast_target::CastTargeting,
    mut ladder: CastLadder,
    mut ui: ResMut<UiEvents>,
) {
    ladder.send(spell_id, &targeting.context(), CastCommit::Spell);
    ladder.press.defer(&mut ui);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The world leave clears the list's product callbacks and its in-flight count (`0x528d0f`
    /// -> `0x5dad10`), and leaves a reagent's (`0x5de100`) waiting.
    #[test]
    fn the_world_leave_clears_the_lists_item_callbacks() {
        let mut world = World::new();
        let mut open = TradeSkillOpen::default();
        open.list.items_pending = 1;
        world.insert_resource(open);
        let mut waits = RecordWaits::default();
        waits.wait(1251, RecordWait::TradeSkillList, false);
        waits.wait(2589, RecordWait::TradeSkillItem, false);
        world.insert_non_send_resource(waits);
        leave_world(&mut world);
        assert_eq!(world.resource::<TradeSkillOpen>().list.items_pending, 0);
        assert_eq!(
            world.non_send_resource::<RecordWaits>().waiting(),
            [(2589, RecordWait::TradeSkillItem)]
        );
    }

    fn spell(effect: u32) -> SpellDisplay {
        SpellDisplay {
            id: 7411,
            name: "Enchanting".into(),
            effects: [effect, 0, 0],
            ..Default::default()
        }
    }

    /// `0x5dd5a0`'s bands, a zero trivialLow read as trivialHigh - 25.
    #[test]
    fn difficulty_bands_on_the_trivial_ranks() {
        assert_eq!(difficulty(1, 0, 40), TradeSkillDifficulty::Optimal);
        assert_eq!(difficulty(20, 0, 40), TradeSkillDifficulty::Medium);
        assert_eq!(difficulty(10, 15, 40), TradeSkillDifficulty::Optimal);
        assert_eq!(difficulty(30, 15, 40), TradeSkillDifficulty::Easy);
        assert_eq!(difficulty(40, 15, 40), TradeSkillDifficulty::Trivial);
        assert_eq!(difficulty(0, 0, 25), TradeSkillDifficulty::Medium);
    }

    /// The `0xa2d288` table with the bag, finger and trinket overrides and the catch-all.
    #[test]
    fn a_products_inventory_type_folds_to_its_slot_bits() {
        assert_eq!(product_slots(5), 1 << 4);
        assert_eq!(product_slots(11), 1 << 10);
        assert_eq!(product_slots(12), 1 << 12);
        assert_eq!(product_slots(18), 1 << 19);
        assert_eq!(product_slots(17), (1 << 15) | (1 << 16));
        assert_eq!(product_slots(0), SLOT_NONEQUIP);
        assert_eq!(product_slots(24), SLOT_NONEQUIP);
    }

    /// An enchant files under its equipped-item requirement.
    #[test]
    fn an_enchant_files_under_its_target_slot() {
        let mut d = spell(53);
        d.equipped_item_class = 4;
        d.equipped_item_inventory_type_mask = 1 << 9; // wrists
        assert_eq!(enchant_slots(&d), 1 << 8);
        d.equipped_item_inventory_type_mask = 1 << 11; // finger: the single bit
        assert_eq!(enchant_slots(&d), 1 << 10);
        d.equipped_item_inventory_type_mask = 0;
        d.equipped_item_subclass_mask = 0x40;
        assert_eq!(enchant_slots(&d), 0x200_0000);
        d.equipped_item_class = 2;
        d.equipped_item_subclass_mask = 1;
        assert_eq!(enchant_slots(&d), 0x80_0000);
        d.equipped_item_subclass_mask = 2;
        assert_eq!(enchant_slots(&d), 0x100_0000);
        d.equipped_item_class = -1;
        assert_eq!(enchant_slots(&d), SLOT_NONEQUIP);
    }

    /// `0x5db6e0`: BasePoints + 1 to BasePoints + DieSides, at least 1.
    #[test]
    fn num_made_reads_effect_zeros_points() {
        let mut d = spell(24);
        d.effect_die_sides[0] = 1;
        assert_eq!(num_made(&d), (1, 1));
        d.effect_base_points[0] = 1;
        d.effect_die_sides[0] = 3;
        assert_eq!(num_made(&d), (2, 4));
        d.effect_base_points[0] = -1;
        d.effect_die_sides[0] = 0;
        assert_eq!(num_made(&d), (1, 1));
    }

    /// The sign goes to `%c` only.
    #[test]
    fn a_stat_line_signs_only_a_c_template() {
        assert_eq!(signed_line("%c%d Stamina", 12, &[]), "+12 Stamina");
        assert_eq!(
            signed_line("Improves hit rating by %d.", 10, &[]),
            "Improves hit rating by 10."
        );
        assert_eq!(
            signed_line("%c%d %s Resistance", -5, &[Arg::S("Fire")]),
            "-5 Fire Resistance"
        );
    }

    /// `0x5db060`: one base64 digit per six rows, a bit per known row the race and class admit.
    #[test]
    fn the_list_link_packs_one_bit_per_known_row() {
        use benilla_formats::{LinkAbility, SkillLineCatalog};
        let run: Vec<LinkAbility> = (0..8)
            .map(|i| LinkAbility {
                spell_id: 100 + i,
                race_mask: if i == 2 { 1 << 1 } else { 0 },
                class_mask: 0,
            })
            .collect();
        let catalog = SkillLineCatalog::from_link_run_for_tests(333, true, run);
        let known = [100, 102, 107].into_iter().collect();
        let link = list_link(
            &spell(benilla_formats::SPELL_EFFECT_TRADE_SKILL),
            333,
            &catalog,
            &known,
            (3, 1, 0xABC),
            (150, 225),
        )
        .unwrap();
        // Row 0 (bit 0) only in the first six: row 2 is race 2's; row 7 is bit 1 of the second.
        assert_eq!(
            link,
            "|cffffd000|Htrade:7411:150:225:ABC:BC|h[Enchanting]|h|r"
        );
        let blocked = SkillLineCatalog::from_link_run_for_tests(333, false, Vec::new());
        assert_eq!(
            list_link(
                &spell(benilla_formats::SPELL_EFFECT_TRADE_SKILL),
                333,
                &blocked,
                &known,
                (3, 1, 1),
                (1, 1)
            ),
            None
        );
    }

    const ROD: u32 = 7421;

    /// Enchanting's `SkillLine.dbc` display.
    fn enchanting_line() -> benilla_formats::SkillLineInfo {
        benilla_formats::SkillLineInfo {
            name: "Enchanting".into(),
            category_id: 11,
            icon: None,
            description: String::new(),
            alternate_verb: String::new(),
            can_link: true,
            costs_id: 0,
        }
    }

    /// The add and the removal look the ability's line up in `SkillLine.dbc`: a line it lacks files
    /// no node (`0x6e7b9f`-`0x6e7bbe`) and its removal neither leaves an array nor rebuilds
    /// (`0x6e7264`-`0x6e7270`).
    #[test]
    fn a_recipe_whose_line_skill_line_lacks_files_nowhere() {
        let (mut app, _rx, _me) = craft_world();
        let world = app.world_mut();
        world.init_resource::<UiEvents>();
        let mut spells = crate::ui_action::Spells::empty_for_tests();
        spells.catalog = benilla_formats::SpellCatalog::from_displays(
            [(
                ROD,
                SpellDisplay {
                    name: "Runed Copper Rod".into(),
                    attributes: SPELL_ATTR_IS_TRADESKILL,
                    ..Default::default()
                },
            )]
            .into(),
        );
        world.insert_resource(spells);
        let lines = |with_line: bool| {
            let c = benilla_formats::SkillLineCatalog::from_spell_lines([(ROD, 333)]);
            SkillLines {
                catalog: std::sync::Arc::new(if with_line {
                    c.with_lines([(333, enchanting_line())])
                } else {
                    c
                }),
            }
        };
        world.insert_resource(lines(false));
        recipe_learned(world, ROD);
        assert_eq!(world.resource::<TradeSkillRecipes>().line(333), None);
        // Filed while the line was known, the removal then finds no line and keeps the array.
        world.insert_resource(lines(true));
        recipe_learned(world, ROD);
        assert_eq!(
            world.resource::<TradeSkillRecipes>().line(333),
            Some(&[ROD][..])
        );
        // The first add rebuilt with no array, which fires nothing (`0x5dd7b6`); the second fired.
        assert_eq!(world.resource::<UiEvents>().names(), ["TRADE_SKILL_UPDATE"]);
        world.insert_resource(lines(false));
        world.insert_resource(UiEvents::default());
        recipe_removed(world, ROD);
        assert_eq!(
            world.resource::<TradeSkillRecipes>().line(333),
            Some(&[ROD][..])
        );
        assert!(world.resource::<UiEvents>().is_empty(), "no rebuild fired");
    }

    /// The whole client headless with us, the Enchanting list holding Runed Copper Rod on the
    /// standard GCD, and a VM in the world.
    fn craft_world() -> (
        App,
        crossbeam_channel::Receiver<crate::net::ClientCommand>,
        Entity,
    ) {
        let mut app = crate::game_plugins::schedule_tests::headless_client();
        let (tx, rx) = crossbeam_channel::unbounded();
        app.insert_resource(NetCommands(tx));
        let world = app.world_mut();
        let me = world
            .spawn((
                SelfPlayer,
                crate::net::Guid(0x10),
                ObjectStore(benilla_protocol::ObjectFields::default()),
            ))
            .id();
        world.resource_mut::<SelfGuid>().0 = Some(0x10);
        world.resource_mut::<GuidIndex>().0.insert(0x10, me);
        // Our own name record, which the login seeds and the rebuild reads (`0x5dd696`).
        world.init_resource::<crate::names::NameCache>();
        world
            .resource_mut::<crate::names::NameCache>()
            .insert_player(0x10, "Me".into(), None);
        world.insert_non_send_resource(benilla_ui::script::UiScript::new().expect("a VM"));
        {
            let mut open = world.resource_mut::<TradeSkillOpen>();
            open.list.open_line(333, 7411);
            open.list.refresh(
                true,
                Some(vec![TradeSkillRow {
                    spell_id: ROD,
                    name: "Runed Copper Rod".into(),
                    difficulty: TradeSkillDifficulty::Trivial,
                    num_available: 5,
                    trivial_high: 100,
                    group: Some(TradeSkillGroup::SubClass {
                        class: 7,
                        subclass: 12,
                        name: Some("Enchanting".into()),
                    }),
                    item_level: 0,
                    inv_slots: 1 << 26,
                }]),
                &|_| true,
            );
            open.list.opened();
        }
        let mut spells = crate::ui_action::Spells::empty_for_tests();
        spells.catalog = benilla_formats::SpellCatalog::from_displays(
            [(
                ROD,
                SpellDisplay {
                    name: "Runed Copper Rod".into(),
                    start_recovery_category: 133,
                    start_recovery_ms: 1500,
                    ..Default::default()
                },
            )]
            .into(),
        );
        world.insert_resource(spells);
        (app, rx, me)
    }

    const DO_ROD: &str = r#"for i = 1, GetNumTradeSkills() do
                       if select(2, GetTradeSkillInfo(i)) ~= "header" then
                           SelectTradeSkill(i)
                           DoTradeSkill(i, 3)
                           GCD = select(2, GetSpellCooldown(7421))
                           REPEAT = GetTradeskillRepeatCount()
                       end
                   end"#;

    /// `DoTradeSkill` casts inside the call (`0x5dacb1` -> `0x80da40`): the caller's next line reads the GCD the press armed and the repeat it set once the cast went (`0x5dacc1`).
    #[test]
    fn do_trade_skill_casts_inside_its_call() {
        let (mut app, rx, _me) = craft_world();
        let world = app.world_mut();
        crate::ui_script::safe_world::lend(world, |s| s.run(DO_ROD))
            .expect("a VM")
            .expect("the chunk runs");
        let sent = rx
            .try_iter()
            .filter(|c| matches!(c, crate::net::ClientCommand::CastSpell { spell_id, .. } if *spell_id == ROD))
            .count();
        assert_eq!(sent, 1, "the cast went out inside the call");
        let script = world.non_send_resource::<benilla_ui::script::UiScript>();
        let gcd: f64 = script.eval("return GCD").expect("the line ran");
        let repeat: i64 = script.eval("return REPEAT").expect("the line ran");
        assert_eq!(gcd, 1.5, "the next line reads the press's GCD");
        assert_eq!(repeat, 3, "the cast went, so the repeat took the count");
    }

    /// The repeat takes the attempt's own verdict (`0x80d9b0`), not what a later `SENT` listener leaves: one that stops the cast still finds the repeat set (`0x5dacbb`).
    #[test]
    fn a_sent_listener_that_stops_the_cast_leaves_the_repeat_set() {
        let (mut app, _rx, _me) = craft_world();
        let world = app.world_mut();
        crate::ui_script::safe_world::lend(world, |s| {
            s.run(
                r#"local f = CreateFrame("Frame")
                   f:RegisterEvent("UNIT_SPELLCAST_SENT")
                   f:SetScript("OnEvent", function() STOPPED = 1; SpellStopCasting() end)"#,
            )
            .expect("the listener registers");
            s.run(DO_ROD)
        })
        .expect("a VM")
        .expect("the chunk runs");
        let script = world.non_send_resource::<benilla_ui::script::UiScript>();
        let stopped: Option<i64> = script.eval("return STOPPED").expect("the read runs");
        let repeat: i64 = script.eval("return REPEAT").expect("the line ran");
        assert_eq!(stopped, Some(1), "the SENT listener ran inside the call");
        assert_eq!(
            repeat, 3,
            "the attempt went, whatever its listener did after"
        );
    }

    /// A refused cast returns false and `DoTradeSkill` sets no repeat (`0x5dacbb`): dead, the attempt refuses with 0x13 and the count stays the default 1.
    #[test]
    fn a_refused_do_trade_skill_sets_no_repeat() {
        use benilla_protocol::field::{FIELD_UNIT_HEALTH, FIELD_UNIT_MAXHEALTH};
        let (mut app, rx, me) = craft_world();
        let world = app.world_mut();
        world
            .entity_mut(me)
            .insert(ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                (FIELD_UNIT_HEALTH, 0),
                (FIELD_UNIT_MAXHEALTH, 100),
            ])));
        crate::ui_script::safe_world::lend(world, |s| s.run(DO_ROD))
            .expect("a VM")
            .expect("the chunk runs");
        assert!(
            !rx.try_iter()
                .any(|c| matches!(c, crate::net::ClientCommand::CastSpell { .. })),
            "nothing went out"
        );
        let repeat: i64 = world
            .non_send_resource::<benilla_ui::script::UiScript>()
            .eval("return REPEAT")
            .expect("the line ran");
        assert_eq!(repeat, 1, "no repeat for a refused cast");
    }

    /// Two casts in one call are two presses: the second reads the first's GCD (`0x80b583`) and the bail ends it (`0x80d34b`, `0x80d36d`).
    #[test]
    fn the_second_cast_in_one_call_reads_the_firsts_gcd() {
        let (mut app, rx, _me) = craft_world();
        let world = app.world_mut();
        crate::ui_script::safe_world::lend(world, |s| {
            s.run(
                r#"for i = 1, GetNumTradeSkills() do
                       if select(2, GetTradeSkillInfo(i)) ~= "header" then
                           SelectTradeSkill(i)
                           DoTradeSkill(i, 1)
                           DoTradeSkill(i, 1)
                       end
                   end"#,
            )
        })
        .expect("a VM")
        .expect("the casts run");
        let sent = rx
            .try_iter()
            .filter(|c| {
                matches!(c, crate::net::ClientCommand::CastSpell { spell_id, .. } if *spell_id == ROD)
            })
            .count();
        assert_eq!(sent, 1, "the second press bailed on the first's GCD");
    }

    /// A start and a go of one craft read in one drain commit the repeat, then spend one of it: the go reads the start's commit (`0x5da0f0` then `0x5da070`).
    #[test]
    fn a_start_and_a_go_in_one_drain_run_the_repeat_in_order() {
        use bevy::ecs::system::RunSystemOnce;
        let (mut app, _rx, me) = craft_world();
        let world = app.world_mut();
        world
            .resource_mut::<TradeSkillOpen>()
            .list
            .set_recast(ROD, 3);
        for kind in [
            CastEventKind::Start { visuals: true },
            CastEventKind::Go { ends_cast: true },
        ] {
            world.write_message(CastEvent {
                entity: me,
                spell_id: ROD,
                kind,
                cast_count: 0,
                seq: 0,
            });
        }
        world
            .run_system_once(drain_trade_skill)
            .expect("the drain runs");
        let left = crate::ui_script::safe_world::lend(world, |s| {
            s.run("SelectTradeSkill(GetFirstTradeSkill())")
                .expect("the select runs");
            s.eval::<i64>("return GetTradeskillRepeatCount()")
        })
        .expect("a VM")
        .expect("the count");
        assert_eq!(left, 2, "the go spent one of the committed three");
    }

    /// The open (`0x5de120`): the line's rebuild fires `TRADE_SKILL_UPDATE` (`0x5de2c3`), then `TRADE_SKILL_SHOW` (`0x5de2e5`), each listener reading the open list; the opener of the open line closes it (`0x5de1eb`).
    #[test]
    fn the_open_rebuilds_then_shows_and_the_same_opener_closes() {
        let (mut app, _rx, _me) = craft_world();
        let world = app.world_mut();
        world.resource_mut::<TradeSkillOpen>().list.shutdown();
        world.init_resource::<UiEvents>();
        world.resource_mut::<PlayerActions>().spells.insert(ROD);
        world.insert_resource(SkillLines {
            catalog: std::sync::Arc::new(
                benilla_formats::SkillLineCatalog::from_spell_lines([(7411, 333), (ROD, 333)])
                    .with_lines([(333, enchanting_line())]),
            ),
        });
        let mut spells = crate::ui_action::Spells::empty_for_tests();
        spells.catalog = benilla_formats::SpellCatalog::from_displays(
            [(
                ROD,
                SpellDisplay {
                    name: "Runed Copper Rod".into(),
                    attributes: SPELL_ATTR_IS_TRADESKILL,
                    effects: [53, 0, 0],
                    ..Default::default()
                },
            )]
            .into(),
        );
        world.insert_resource(spells);
        // Our player's add files the recipe under its line (`0x6e7c07`).
        recipe_learned(world, ROD);
        crate::ui_script::safe_world::lend(world, |s| {
            s.run(
                r#"SEEN = {}
                   local f = CreateFrame("Frame")
                   f:RegisterEvent("TRADE_SKILL_UPDATE")
                   f:RegisterEvent("TRADE_SKILL_SHOW")
                   f:RegisterEvent("TRADE_SKILL_CLOSE")
                   f:SetScript("OnEvent", function(_, e)
                       SEEN[#SEEN + 1] = e .. ":" .. GetNumTradeSkills() .. ":" .. (GetTradeSkillInfo(2) or "nil")
                   end)"#,
            )
        })
        .expect("a VM")
        .expect("the listener registers");
        set_trade_skill(world, 7411);
        crate::ui_script::live::fire_handler_events(world);
        set_trade_skill(world, 7411);
        crate::ui_script::live::fire_handler_events(world);
        let seen: Vec<String> = world
            .non_send_resource::<benilla_ui::script::UiScript>()
            .eval("return SEEN")
            .expect("the record");
        assert_eq!(
            seen,
            [
                "TRADE_SKILL_UPDATE:2:Runed Copper Rod",
                "TRADE_SKILL_SHOW:2:Runed Copper Rod",
                "TRADE_SKILL_CLOSE:2:Runed Copper Rod",
            ]
        );
        assert_eq!(world.resource::<TradeSkillOpen>().line(), None);
    }
}
