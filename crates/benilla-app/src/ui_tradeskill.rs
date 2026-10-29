//! The crafting book feed: the client-built TradeSkill window around `benilla_ui::script`'s
//! `tradeskill` module. There is no wire: a profession opener (`SPELL_EFFECT_TRADE_SKILL` in
//! `Effect[0]`) never reaches the send, as the spell-cast entry (`0x80d0ab`) opens the window
//! client-side, every profession through the trade skill's open (`0x5de120`), and
//! [`TradeSkillOpens`] is that intercept here.
//!
//! The book lists each known spell with `SPELL_ATTR_IS_TRADESKILL` (`0x20`, which also hides it
//! from the spellbook) whose `SkillLineAbility` row joins the open line; the engine groups, sorts
//! and filters it (`0x5dd5a0`).

use std::collections::HashMap;

use bevy::prelude::*;

use benilla_formats::{SpellDisplay, SPELL_ATTR_IS_TRADESKILL};
use benilla_protocol::messages::{ItemInfo, PLAYER_SKILL_SLOTS};
use benilla_protocol::{SessionEvent, SessionEventKind};
use benilla_ui::script::{
    TradeSkillDifficulty, TradeSkillGroup, TradeSkillReagent, TradeSkillRecipe, TradeSkillState,
    UiScript,
};
use benilla_ui::strings::Arg;

use crate::creature_anim::{CastEvent, CastEventKind};
use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::net::{NetCommands, NetHandlerApp, ObjectStore, Objects, SelfGuid, SelfPlayer};
use crate::spell::{cast_target, CastCommit, CastLadder};
use crate::ui_action::{PlayerActions, Spells};
use crate::ui_items::{count_of, item_icon, tool_categories, InventoryScope};
use crate::ui_script::UiInput;
use crate::ui_spellbook::SkillLines;
use crate::ui_unit::UnitFeed;
use benilla_assets::{AssetSet, LockRecover, WorldAssets};

/// `SPELL_EFFECT_CREATE_ITEM`, `_CREATE_RANDOM_ITEM` and `_CREATE_ITEM_2`: the effects a recipe
/// makes its product with (`0x5dd5a0`).
const CREATE_ITEM_EFFECTS: [u32; 3] = [24, 59, 157];

/// Inventory type to slot-filter bits (`0xa2d288`).
const INVENTORY_TYPE_SLOTS: [u32; 29] = [
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

/// Opener spell ids (effect 47) that `send_spell_cast` intercepted, opened by
/// [`open_trade_skill`] the same frame.
#[derive(Resource, Default)]
pub(crate) struct TradeSkillOpens(pub(crate) Vec<u32>);

/// The open book: its skill line and opener, and the events an open or an app-side close owes.
/// The session end clears it: a logout replaces the VM without running the old `OnHide`.
#[derive(Resource, Default)]
pub(crate) struct TradeSkillOpen {
    pub(crate) line: Option<u32>,
    /// The opener's spell id (`0xc235a0`), the list link's.
    spell_id: u32,
    /// An opener ran: `TRADE_SKILL_SHOW` is owed after the push (`0x5de2e5`).
    show: bool,
    /// The book closed outside Lua: `TRADE_SKILL_CLOSE` is owed (`0x5da5df`).
    close: bool,
}

/// `SpellFocusObject.dbc`, the names on a "Requires: Anvil" line.
#[derive(Resource)]
pub(crate) struct SpellFocus {
    pub(crate) catalog: benilla_formats::SpellFocusCatalog,
}

pub(crate) struct UiTradeSkillPlugin;

impl Plugin for UiTradeSkillPlugin {
    fn build(&self, app: &mut App) {
        app.net_handler(SessionEventKind::Disconnected, on_session_end);
        app.init_resource::<TradeSkillOpens>()
            .init_resource::<TradeSkillOpen>()
            .add_systems(Startup, load_spell_focus.after(AssetSet::Open))
            .add_systems(
                Update,
                (
                    // Opens before the feed, so an opener shows the window this frame; the
                    // drain after the input pass, so a Create click casts this frame.
                    open_trade_skill
                        .before(feed_trade_skill)
                        .after(benilla_world::schedule::WorldStage::Net),
                    feed_trade_skill.in_set(UnitFeed),
                    drain_trade_skill.after(UiInput),
                ),
            );
    }
}

/// The book and any queued opener end with the session.
fn on_session_end(
    In(_): In<SessionEvent>,
    mut opens: ResMut<TradeSkillOpens>,
    mut open: ResMut<TradeSkillOpen>,
) {
    opens.0.clear();
    *open = TradeSkillOpen::default();
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

/// Open the book for each intercepted opener, on its `SkillLineAbility` row's line; the opener
/// of the line already open closes it instead (`0x5de120`).
fn open_trade_skill(
    mut opens: ResMut<TradeSkillOpens>,
    mut open: ResMut<TradeSkillOpen>,
    skill_lines: Option<Res<SkillLines>>,
    self_q: Query<&ObjectStore, With<crate::net::SelfPlayer>>,
) {
    let (race, class) = crate::spell::race_class(self_q.iter().next());
    for spell_id in opens.0.drain(..) {
        let Some(skill_lines) = skill_lines.as_deref() else {
            warn!("ui_tradeskill: opener {spell_id} before SkillLineAbility loaded — dropped");
            continue;
        };
        let Some(line) = skill_lines.catalog.spell_to_line(spell_id, race, class) else {
            warn!("ui_tradeskill: opener {spell_id} has no SkillLineAbility row — dropped");
            continue;
        };
        if open.line == Some(line) {
            debug!("ui_tradeskill: opener {spell_id} closes skill line {line}");
            open.line = None;
            open.close = true;
            continue;
        }
        debug!("ui_tradeskill: opener {spell_id} opens skill line {line}");
        open.line = Some(line);
        open.spell_id = spell_id;
        open.show = true;
    }
}

/// A skill line's `(rank, max, permanent bonus)` off `PLAYER_SKILL_INFO`.
fn skill_rank(store: &ObjectStore, skill_id: u32) -> (u32, u32, i32) {
    for i in 0..PLAYER_SKILL_SLOTS {
        if let Some(s) = store.0.player_skill(i) {
            if u32::from(s.skill_id) == skill_id {
                return (
                    u32::from(s.value),
                    u32::from(s.max),
                    i32::from(s.perm_bonus),
                );
            }
        }
    }
    (0, 0, 0)
}

/// The skill's value and max rank (`0x51a250`, `0x594710`): a nonzero rank plus the permanent
/// bonus.
fn with_bonus(rank: u32, bonus: i32) -> u32 {
    if rank == 0 {
        0
    } else {
        rank.saturating_add_signed(bonus)
    }
}

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

/// A recipe's parsed description and folded search texts, cached by spell id and rebuilt when
/// the token inputs move or another of its templates lands.
#[derive(Clone, Default)]
struct RecipeText {
    /// How many of the product, reagent and tool templates were cached when it was built.
    templates: Option<usize>,
    description: Option<String>,
    search_text: Vec<String>,
}

/// What a recipe's `$` tokens read of the player, and the line.
type TokenReads = (
    u32,
    crate::spell::SkillSnapshot,
    u8,
    crate::spell::text_caster::Operands,
);

/// The book's catalogs, one system parameter.
#[derive(bevy::ecs::system::SystemParam)]
struct BookData<'w> {
    spells: Option<Res<'w, Spells>>,
    spell_mods: Res<'w, crate::spell::SpellModifiers>,
    skill_lines: Option<Res<'w, SkillLines>>,
    focus: Option<Res<'w, SpellFocus>>,
    icons: Option<Res<'w, ItemDisplays>>,
    subclasses: Option<Res<'w, crate::ui_items::ItemSubClasses>>,
    totem_categories: Res<'w, crate::ui_totem::TotemCategories>,
    cooldowns: Res<'w, crate::spell::Cooldowns>,
    clock: Res<'w, crate::ui_script::UiClock>,
}

/// `GetTradeSkillListLink` (`0x5db060`): the opener's `|Htrade:` link, one bit per row of the
/// line's link run, set for a known spell the player's race and class may learn.
fn list_link(
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

/// Build the book: the known recipes of the open line, banded against the current rank.
#[allow(clippy::too_many_arguments)] // a Bevy system's full input set
fn feed_trade_skill(
    script: Option<NonSendMut<UiScript>>,
    mut open: ResMut<TradeSkillOpen>,
    actions: Res<PlayerActions>,
    data: BookData,
    self_store: Query<&ObjectStore, With<SelfPlayer>>,
    self_auras: Query<&crate::unit_auras::UnitAuras, With<SelfPlayer>>,
    caster_sources: crate::spell::text_caster::CasterSources,
    self_guid: Res<SelfGuid>,
    objects: Objects,
    items: Res<Items>,
    commands: Res<NetCommands>,
    mut last: Local<crate::ui_script::VmMemo<Option<TradeSkillState>>>,
    mut texts: Local<crate::ui_script::VmMemo<HashMap<u32, RecipeText>>>,
    mut last_reads: Local<crate::ui_script::VmMemo<Option<TokenReads>>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let me = self_store.single().ok();
    let reads = me.zip(open.line).map(|(s, line)| {
        (
            line,
            crate::spell::skill_snapshot(Some(s)),
            s.0.unit_gender().unwrap_or(0),
            crate::spell::text_caster::Operands::of(Some(s), &items),
        )
    });
    if *last_reads.get(&script) != reads || data.spell_mods.is_changed() || caster_sources.changed()
    {
        texts.get(&script).clear();
    }
    *last_reads.get(&script) = reads;

    let fresh = (|| -> Option<TradeSkillState> {
        let line = open.line?;
        let spells = data.spells.as_deref()?;
        let skill_lines = data.skill_lines.as_deref()?;
        let store = me?;
        let (raw_rank, raw_max, bonus) = skill_rank(store, line);
        let shown = (with_bonus(raw_rank, bonus), with_bonus(raw_max, bonus));
        let line_info = skill_lines.catalog.line(line);
        let line_name = line_info.map_or_else(|| format!("Skill {line}"), |l| l.name.clone());
        let alt_verb = line_info
            .map(|l| l.alternate_verb.clone())
            .unwrap_or_default();

        let global = |key: &str| benilla_ui::strings::global(script.lua(), key);
        let skill =
            |id| crate::spell::spell_skill_value(Some(store), Some(&skill_lines.catalog), id);
        let operands = crate::spell::text_caster::Operands::of(Some(store), &items);
        let cost =
            |d: &SpellDisplay| crate::spell::usable::power_cost(d, store, &data.spell_mods) as i32;
        let text_caster = crate::spell::text_caster::untracked(
            store,
            (&actions.spells, self_auras.single().ok()),
            &operands,
            &data.spell_mods,
            &skill,
            &cost,
        );
        let ctx = benilla_formats::TokenContext {
            durations: &spells.durations,
            radii: &spells.radii,
            ranges: Some(&spells.ranges),
            variables: Some(spells.catalog.description_variables()),
            skill: &skill,
            lookup: &|id| spells.catalog.get(id),
            mods: Some(&*data.spell_mods),
            unmodified_points: false,
            gender: &|| store.0.unit_gender().unwrap_or(0),
            home_area: &|| None,
            caster: Some(&text_caster),
            global: &global,
            printf: &crate::ui_script::token_printf,
        };
        let spell_text = |id: u32| {
            let d = spells.catalog.get(id)?;
            let text = d.description.as_deref().filter(|t| !t.is_empty())?;
            Some(benilla_formats::substitute(text, d, &ctx)).filter(|t| !t.is_empty())
        };
        // The totem category search walks mask 7 (`0x7541f0`): worn gear, bags and backpack.
        let carried_tools = tool_categories(
            &store.0,
            &objects,
            &items,
            &commands,
            InventoryScope::DEFAULT,
        );
        let (anchor, ui_now) = (data.clock.anchor, data.clock.ui_now);
        let texts = texts.get(&script);

        let mut recipes = Vec::new();
        // A recipe files under its line for our race and class (`0x6e7b00`).
        let (race, class) = crate::spell::race_class(Some(store));
        for &spell_id in &actions.spells {
            let Some(d) = spells.catalog.get(spell_id) else {
                continue;
            };
            if d.attributes & SPELL_ATTR_IS_TRADESKILL == 0 {
                continue;
            }
            let Some(sla) = skill_lines
                .catalog
                .ability(spell_id, race, class)
                .filter(|sla| sla.skill_id == line)
            else {
                continue;
            };
            let mut templates = 0;

            // Reagents: every nonzero slot, counted over the default inventory walk (mask 0).
            let mut reagents = Vec::new();
            let mut num_available: i32 = -1;
            for &(entry, need) in d.reagents.iter().filter(|&&(e, _)| e != 0) {
                let have = count_of(&store.0, &objects, entry, InventoryScope::DEFAULT);
                let template = items.template(entry, 0, &commands);
                templates += usize::from(template.is_some());
                let (name, icon) = match template {
                    Some(t) => (
                        Some(t.name.clone()),
                        item_icon(data.icons.as_deref(), t.display_info_id),
                    ),
                    None => (None, None),
                };
                if need != 0 && num_available != 0 {
                    let can = (have / need) as i32;
                    if num_available == -1 || num_available >= can {
                        num_available = can;
                    }
                }
                reagents.push(TradeSkillReagent {
                    item: entry,
                    name,
                    icon,
                    need,
                    have,
                });
            }

            // The product, its header and slot bits; a template in flight holds the list back.
            let creates_item = CREATE_ITEM_EFFECTS.contains(&d.effects[0]);
            let product_item = d.effect_item_type[0];
            let product = (product_item != 0)
                .then(|| items.template(product_item, 0, &commands))
                .flatten();
            templates += usize::from(product.is_some());
            let (group, item_level, inv_slots) = if creates_item && product_item != 0 {
                match product {
                    Some(t) => (
                        Some(TradeSkillGroup::SubClass {
                            class: t.class,
                            subclass: t.subclass,
                            name: data
                                .subclasses
                                .as_deref()
                                .and_then(|sc| sc.0.name(t.class, t.subclass))
                                .map(str::to_string),
                        }),
                        t.item_level,
                        product_slots(t.inventory_type),
                    ),
                    None => (None, 0, SLOT_NONEQUIP),
                }
            } else {
                (Some(TradeSkillGroup::Verb), 0, enchant_slots(d))
            };
            let icon = if creates_item && product_item != 0 {
                product.and_then(|t| item_icon(data.icons.as_deref(), t.display_info_id))
            } else {
                d.icon.clone()
            };

            // Tools in `0x5de620`'s order: the focus, always met, then the tool items and the
            // tool categories. A tool whose template has not landed is left out.
            let focus = (d.requires_spell_focus != 0)
                .then(|| {
                    data.focus
                        .as_deref()
                        .and_then(|f| f.catalog.name(d.requires_spell_focus))
                })
                .flatten();
            let mut tool_items = Vec::new();
            for &t in d.totems.iter().filter(|&&t| t != 0) {
                let have = count_of(&store.0, &objects, t, InventoryScope::DEFAULT) > 0;
                if let Some(info) = items.template(t, 0, &commands) {
                    templates += 1;
                    tool_items.push((info.name.clone(), have));
                }
            }
            let tool_kinds: Vec<(String, bool)> = d
                .totem_categories
                .iter()
                .filter(|&&c| c != 0)
                .filter_map(|&c| {
                    let row = data.totem_categories.0.get(c)?;
                    let have = data
                        .totem_categories
                        .0
                        .covered_by(c, carried_tools.iter().copied());
                    Some((row.name.clone(), have))
                })
                .collect();

            let text = texts.entry(spell_id).or_default();
            if text.templates != Some(templates) {
                let description = spell_text(spell_id);
                let mut search = Vec::new();
                if let Some(t) = product {
                    product_texts(t, &global, &spell_text, &mut search);
                }
                search.push(d.name.clone());
                search.extend(reagents.iter().filter_map(|r| r.name.clone()));
                search.extend(tool_items.iter().map(|(n, _)| n.clone()));
                search.extend(tool_kinds.iter().map(|(n, _)| n.clone()));
                search.extend(focus.map(str::to_string));
                search.extend(description.iter().cloned());
                // `CaseAccentInsensitiveParse` for enUS (locale 0) is a lowercase fold.
                *text = RecipeText {
                    templates: Some(templates),
                    description,
                    search_text: search.iter().map(|s| s.to_lowercase()).collect(),
                };
            }

            let mut tools: Vec<(String, bool)> =
                focus.map(|f| (f.to_string(), true)).into_iter().collect();
            tools.extend(tool_items);
            tools.extend(tool_kinds);
            let (min_made, max_made) = num_made(d);
            let cooldown_end = data
                .cooldowns
                .info(spell_id, 0, Some(d), anchor)
                .ui_triple(anchor, ui_now)
                .filter(|&(_, _, enabled)| enabled)
                .map(|(start_ms, duration_ms, _)| {
                    (start_ms + i64::from(duration_ms)) as f64 / 1000.0
                });
            recipes.push(TradeSkillRecipe {
                spell_id,
                name: d.name.clone(),
                difficulty: difficulty(raw_rank, sla.trivial_low, sla.trivial_high),
                num_available,
                trivial_high: sla.trivial_high,
                group,
                item_level,
                inv_slots,
                creates_item,
                product_item,
                required_level: product.map(|t| t.required_level),
                icon,
                min_made,
                max_made,
                cooldown_end,
                description: text.description.clone(),
                search_text: text.search_text.clone(),
                reagents,
                tools,
            });
        }
        let who = (
            store.0.unit_race().unwrap_or(0),
            store.0.unit_class().unwrap_or(0),
            self_guid.0.unwrap_or(0),
        );
        let list_link = spells.catalog.get(open.spell_id).and_then(|opener| {
            list_link(
                opener,
                line,
                &skill_lines.catalog,
                &actions.spells,
                who,
                shown,
            )
        });
        Some(TradeSkillState {
            line,
            line_name,
            alt_verb,
            rank: shown.0,
            max_rank: shown.1,
            list_link,
            recipes,
        })
    })();

    let last = last.get(&script);
    let show = std::mem::take(&mut open.show);
    let close = std::mem::take(&mut open.close);
    if fresh != *last {
        script.set_trade_skill(fresh.clone());
        // The link verbs never query, so the feed asks for the missing templates here.
        if let Some(f) = &fresh {
            script.ask_item_templates(f.recipes.iter().flat_map(|r| {
                std::iter::once(r.product_item).chain(r.reagents.iter().map(|re| re.item))
            }));
        }
        // Every rebuild fires TRADE_SKILL_UPDATE (`0x5de0d5`).
        if fresh.is_some() && !show {
            script.fire_event("TRADE_SKILL_UPDATE", vec![]);
        }
    }
    if show && fresh.is_some() {
        debug!(
            "ui_tradeskill: window opens — {} recipe(s)",
            fresh.as_ref().map_or(0, |f| f.recipes.len())
        );
        script.fire_event("TRADE_SKILL_UPDATE", vec![]);
        if script.trade_skill_has_entries() {
            script.fire_event("TRADE_SKILL_SHOW", vec![]);
        }
    }
    if close {
        script.fire_event("TRADE_SKILL_CLOSE", vec![]);
    }
    *last = fresh;
}

/// Drain the Lua intents and run the engine's repeat on our own cast edges: a start commits a
/// pending repeat, a go may cast again, a failure stops it.
fn drain_trade_skill(
    script: Option<NonSendMut<UiScript>>,
    mut open: ResMut<TradeSkillOpen>,
    mut cast_events: MessageReader<CastEvent>,
    targeting: cast_target::CastTargeting,
    mut ladder: CastLadder,
) {
    let Some(mut script) = script else {
        return;
    };

    for spell_id in script.take_trade_skill_casts() {
        if open.line.is_none() {
            debug!("ui_tradeskill: DoTradeSkill({spell_id}) with no open book — ignored");
            continue;
        }
        debug!("ui_tradeskill: DoTradeSkill({spell_id})");
        ladder.send(spell_id, &targeting.context(), CastCommit::Spell);
    }

    let self_entity = ladder.self_player.single().ok().map(|(e, _)| e);
    for ev in cast_events.read() {
        if Some(ev.entity) != self_entity {
            continue;
        }
        match ev.kind {
            CastEventKind::Start => script.trade_skill_cast_started(ev.spell_id),
            CastEventKind::Go => {
                if script.trade_skill_cast_done(ev.spell_id) {
                    debug!("ui_tradeskill: recast {}", ev.spell_id);
                    ladder.send(ev.spell_id, &targeting.context(), CastCommit::Spell);
                }
            }
            CastEventKind::Fail => script.trade_skill_cast_failed(ev.spell_id),
            _ => {}
        }
    }

    if script.take_trade_skill_close() {
        debug!("ui_tradeskill: client-side close (no packet)");
        open.line = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// Driven through the real handler registration.
    #[test]
    fn the_session_end_closes_the_book() {
        let mut app = App::new();
        app.add_plugins(UiTradeSkillPlugin);
        app.world_mut().resource_mut::<TradeSkillOpen>().line = Some(197);

        crate::net::handlers::dispatch(
            app.world_mut(),
            vec![benilla_protocol::SessionEvent::Disconnected {
                reason: "logged out".into(),
                end: benilla_protocol::SessionEnd::LoggedOut,
            }],
        );

        assert_eq!(app.world().resource::<TradeSkillOpen>().line, None);
    }
}
