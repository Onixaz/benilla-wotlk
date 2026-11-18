//! The per-frame feeds into the VM: the container snapshots built from the player's descriptor,
//! and the item-tooltip views every window reads.

use std::collections::{HashMap, HashSet};

use bevy::prelude::*;

use benilla_protocol::ItemInfo;
use benilla_ui::script::{ContainerSlot, UiScript};

use crate::entities::{ItemDisplays, FALLBACK_ITEM_ICON};
use crate::items::Items;
use crate::net::{NetCommands, ObjectStore, Objects, SelfPlayer};

use super::view_reads::{Inputs, Moved, Seen, ViewReads};
use super::{find_equip_slot, BAGS, BAG_SLOT_FIRST, BANK_BAGS, BANK_BAG_SLOT_FIRST};

/// A spell's `$`-substituted description, for item trigger and set-bonus lines. An empty one yields
/// `None` and the caller drops the whole line, prefix included: the item builder expands it, tests
/// the first byte and on zero skips the trigger block, so an undescribed spell such as a key's
/// `Opening` prints no line.
fn spell_desc_text(
    spells: Option<&crate::ui_action::Spells>,
    id: u32,
    // The player's skill in a spell's line, the `$`-tokens' level (`TokenContext::skill`).
    skill: &dyn Fn(u32) -> u32,
    home_area: Option<&str>,
    // The active player's `UNIT_FIELD_BYTES_0` byte 2, the `$g`/`$G` branch's input.
    gender: &dyn Fn() -> u8,
    mods: Option<&crate::spell::SpellModifiers>,
    // The VM's strings for the keyed `$d`/`$s` tokens.
    global: &dyn Fn(&str) -> Option<String>,
    // The player's known spells, auras and fields, which `$?` and `${…}` read.
    caster: Option<&dyn benilla_formats::Caster>,
) -> Option<String> {
    let sp = spells?;
    let d = sp.catalog.get(id)?;
    match d.description.as_deref().filter(|t| !t.is_empty()) {
        // The builder tests the expanded text, which can be empty from a non-empty template.
        Some(desc) => {
            let ctx = benilla_formats::TokenContext {
                durations: &sp.durations,
                radii: &sp.radii,
                ranges: Some(&sp.ranges),
                variables: Some(sp.catalog.description_variables()),
                skill,
                lookup: &|i| sp.catalog.get(sp.for_difficulty(i)),
                mods: mods.map(|m| m as &dyn benilla_formats::SpellMods),
                unmodified_points: false,
                gender,
                home_area: &|| home_area,
                caster,
                global,
                printf: &crate::ui_script::token_printf,
            };
            Some(benilla_formats::substitute(desc, d, &ctx)).filter(|t| !t.is_empty())
        }
        None => None,
    }
}

/// What the `$`-tokens read of the player beside the modifiers, the known set and the auras: the
/// skills the per-level terms scale by, the gender byte `$g`/`$G` branch on, and the expression
/// operands off its fields.
pub(super) type TokenReads = (
    crate::spell::SkillSnapshot,
    u8,
    crate::spell::text_caster::Operands,
);

fn token_reads(me: Option<&ObjectStore>, items: &Items) -> TokenReads {
    let gender = me.and_then(|s| s.0.unit_gender()).unwrap_or(0);
    (
        crate::spell::skill_snapshot(me),
        gender,
        crate::spell::text_caster::Operands::of(me, items),
    )
}

/// The held item views a change of the player can move.
#[derive(Default)]
pub(crate) struct ModSensitive {
    /// Every view with a `$` in a trigger text: due when the modifiers, the skills or the gender
    /// move.
    all: HashSet<u32>,
    /// Of those, the ones whose tokens read the player through the caster, and what each read.
    reads: HashMap<u32, ViewReads>,
}

impl ModSensitive {
    /// File the view just built for `id`: whether a trigger text has a `$`, and what it read.
    fn hold(&mut self, id: u32, has_tokens: bool, reads: ViewReads) {
        if has_tokens {
            self.all.insert(id);
        } else {
            self.all.remove(&id);
        }
        if has_tokens && !reads.is_empty() {
            self.reads.insert(id, reads);
        } else {
            self.reads.remove(&id);
        }
    }
}

/// Only descriptions with substitution tokens can change when the caster's spell mods change.
fn spell_has_mod_tokens(spells: Option<&crate::ui_action::Spells>, id: u32) -> bool {
    spells
        .and_then(|s| s.catalog.get(id))
        .and_then(|d| d.description.as_deref())
        .is_some_and(|desc| desc.contains('$'))
}

fn item_has_mod_tokens(t: &ItemInfo, spells: Option<&crate::ui_action::Spells>) -> bool {
    t.spells
        .iter()
        .any(|slot| slot.spell_id != 0 && spell_has_mod_tokens(spells, slot.spell_id))
}

fn bonuses_have_mod_tokens(
    bonuses: &[(u32, u32)],
    spells: Option<&crate::ui_action::Spells>,
) -> bool {
    bonuses
        .iter()
        .any(|&(_, spell)| spell_has_mod_tokens(spells, spell))
}

/// The tooltip's "N Charges" count, 0 for no line. The builder reads a template 0 as `-1`, prints
/// nothing for `-1` and otherwise the absolute value: food's `-1` shows no line, a `-5` pool "5
/// Charges". It prints per spell slot; one line here, as no vmangos `item_template` row has two
/// pools.
fn charges_count(spells: &[benilla_protocol::messages::ItemSpellEntry]) -> i32 {
    spells
        .iter()
        .find(|s| s.spell_id != 0 && s.charges != 0 && s.charges != -1)
        .map(|s| s.charges.abs())
        .unwrap_or(0)
}

/// A reputation rank (0..=7) to `FACTION_STANDING_LABEL{rank+1}`, the unsuffixed key the reference
/// builds, from the player's own `GlobalStrings.lua`.
fn standing_label(rank: u32, get: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    get(&format!("FACTION_STANDING_LABEL{}", rank.min(7) + 1))
}

/// An item template's tooltip view: the wire fields plus the strings only the app resolves, the
/// trigger-spell text and the skill, spell and reputation requirements.
fn template_view(
    t: &ItemInfo,
    spells: Option<&crate::ui_action::Spells>,
    mods: Option<&crate::spell::SpellModifiers>,
    skill: &dyn Fn(u32) -> u32,
    skill_lines: Option<&benilla_formats::SkillLineCatalog>,
    home_area: Option<&str>,
    // The active player's gender byte, the trigger text's `$g`/`$G` branch.
    gender: &dyn Fn() -> u8,
    factions: Option<&benilla_formats::FactionCatalog>,
    sub_classes: Option<&benilla_formats::ItemSubClassCatalog>,
    classes: Option<&benilla_formats::ItemClassCatalog>,
    icons: Option<&ItemDisplays>,
    // The VM's own `GlobalStrings.lua`, for the reputation-requirement line and the `$`-engine's
    // keyed tokens.
    get: &dyn Fn(&str) -> Option<String>,
    caster: Option<&dyn benilla_formats::Caster>,
    // The socket bonus's name.
    enchants: Option<&crate::items::Enchants>,
    // `GetItemUniqueness`'s quantity.
    limits: Option<&crate::ui_trade::ItemLimitCategories>,
) -> benilla_ui::script::ItemTemplateView {
    let spell_name = |id: u32| -> Option<String> {
        spells
            .and_then(|s| s.catalog.get(id))
            .map(|sd| sd.name.clone())
    };
    let spell_text =
        |id: u32| spell_desc_text(spells, id, skill, home_area, gender, mods, get, caster);
    benilla_ui::script::ItemTemplateView {
        name: t.name.clone(),
        quality: t.quality,
        class: t.class,
        subclass: t.subclass,
        inventory_type: t.inventory_type,
        proficiency_alt: sub_classes.and_then(|c| c.proficiency_alt(t.class, t.subclass)),
        hide_subclass: sub_classes.is_some_and(|c| c.hides_name(t.class, t.subclass)),
        // `GetItemInfo`'s type pair: the subclass is `name()`, verbose name first, so a one-handed
        // sword is "One-Handed Swords" to an addon and "Sword" on the tooltip.
        item_type: classes.and_then(|c| c.name(t.class)).map(str::to_string),
        item_sub_type: sub_classes
            .and_then(|c| c.name(t.class, t.subclass))
            .map(str::to_string),
        // The tooltip's spelling: the display name alone.
        sub_class_display: sub_classes
            .and_then(|c| c.display_name(t.class, t.subclass))
            .map(str::to_string),
        // A scaling armor piece's alternate names, leather (2) and mail (3) (`0x628930`).
        scaling_armor_names: if t.scaling_stat_value != 0 && t.class == 4 {
            [2, 3].map(|sub| {
                sub_classes
                    .and_then(|c| c.display_name(4, sub))
                    .map(str::to_string)
            })
        } else {
            Default::default()
        },
        flags: t.flags,
        bonding: t.bonding,
        max_count: t.max_count,
        // The stack size, `GetItemInfo`'s `itemStackCount`; `max_count` is the unique cap.
        stackable: t.stackable,
        // `GetItemInfo`'s `itemTexture`, the bag slots' `ItemDisplayInfo.dbc` icon.
        icon: crate::ui_items::item_icon(icons, t.display_info_id),
        start_quest: t.start_quest,
        container_slots: t.container_slots,
        stats: t.stats.clone(),
        damages: t.damages.iter().map(|d| (d.min, d.max, d.school)).collect(),
        delay_ms: t.delay_ms,
        armor: t.armor,
        block: t.block,
        resistances: t.resistances,
        max_durability: t.max_durability,
        required_level: t.required_level,
        item_level: t.item_level,
        allowable_class: t.allowable_class,
        allowable_race: t.allowable_race,
        required_skill: t.required_skill,
        required_skill_rank: t.required_skill_rank,
        required_skill_name: (t.required_skill != 0)
            .then(|| {
                skill_lines
                    .and_then(|sl| sl.line(t.required_skill))
                    .map(|l| l.name.clone())
            })
            .flatten(),
        required_spell: t.required_spell,
        required_spell_name: (t.required_spell != 0)
            .then(|| spell_name(t.required_spell))
            .flatten(),
        required_honor_rank: t.required_honor_rank,
        required_city_rank: t.required_city_rank,
        // `ITEM_REQ_REPUTATION`, the reference's key for this line: faction, then standing; an
        // install missing a key shows no line.
        required_rep_line: (t.required_rep_faction != 0)
            .then(|| {
                let faction = factions.and_then(|c| c.faction_name(t.required_rep_faction))?;
                let standing = standing_label(t.required_rep_rank, get)?;
                Some(benilla_ui::strings::fill(
                    &get("ITEM_REQ_REPUTATION")?,
                    &[
                        benilla_ui::strings::Arg::S(faction),
                        benilla_ui::strings::Arg::S(&standing),
                    ],
                ))
            })
            .flatten(),
        required_rep_faction: t.required_rep_faction,
        required_rep_rank: t.required_rep_rank,
        lock_id: t.lock_id,
        spell_triggers: t
            .spells
            .iter()
            .filter(|s| s.spell_id != 0)
            .filter_map(|s| spell_text(s.spell_id).map(|n| (s.trigger, s.spell_id, n)))
            .collect(),
        charges: charges_count(&t.spells),
        description: t.description.clone(),
        page_text: t.page_text,
        sell_price: t.sell_price,
        item_set: t.item_set,
        random_property: t.random_property,
        recipe_creates: recipe_creates(t, spells),
        taught: taught_spell(t, spells),
        item_spells: t
            .spells
            .iter()
            .filter(|s| s.spell_id != 0)
            .map(|s| benilla_ui::script::ItemSpellView {
                spell: s.spell_id,
                trigger: s.trigger,
                charges: s.charges,
            })
            .collect(),
        bag_family: t.bag_family,
        scaling_distribution: t.scaling_stat_distribution,
        scaling_value: t.scaling_stat_value,
        armor_damage_modifier: t.armor_damage_modifier,
        item_limit: limits
            .and_then(|l| l.0.get(t.item_limit_category))
            .map(|c| benilla_ui::script::ItemLimitView {
                category: c.id,
                name: c.name.clone(),
                quantity: c.quantity,
                flags: c.flags,
            }),
        socket_colors: t.socket_colors,
        socket_bonus: t.socket_bonus,
        socket_bonus_name: (t.socket_bonus != 0)
            .then(|| enchants.and_then(|e| e.0.name(t.socket_bonus)))
            .flatten()
            .map(str::to_string),
        gem_properties: t.gem_properties,
        gem: enchants
            .and_then(|e| e.1.gem(t.gem_properties))
            .map(|(enchant, gem_type)| benilla_ui::script::GemView {
                gem_type,
                enchant,
                enchant_name: enchants.and_then(|e| e.0.name(enchant)).map(str::to_string),
                condition: enchants
                    .and_then(|e| e.0.condition(enchant))
                    .map(crate::items::condition_view),
                requirement: enchants
                    .and_then(|e| e.0.requirement(enchant))
                    .map(crate::items::requirement_view),
            }),
    }
}

/// The item a recipe teaches to make (`IsDressableItem` `0x517530`): a trigger spell whose first
/// effect is learn-spell (36), teaching its trigger spell, or else the item's own learn-trigger
/// (6) spell, whose first effect creates (24) an item.
pub(crate) fn recipe_creates(t: &ItemInfo, spells: Option<&crate::ui_action::Spells>) -> u32 {
    let Some(catalog) = spells.map(|s| &s.catalog) else {
        return 0;
    };
    t.spells
        .iter()
        .filter_map(|s| catalog.get(s.spell_id))
        .filter(|sd| sd.effects[0] == 36)
        .find_map(|sd| {
            let taught = match sd.effect_trigger_spell[0] {
                0 => t.spells.iter().find(|s| s.trigger == 6)?.spell_id,
                id => id,
            };
            let taught = catalog.get(taught).filter(|l| l.effects[0] == 24)?;
            Some(taught.effect_item_type[0]).filter(|&id| id != 0)
        })
        .unwrap_or(0)
}

/// The spell a learn item teaches and whether its pet learns it (`0x62b3f9`..`0x62b491`): block 0's
/// spell must learn (36), teaching its first trigger spell, else the first learn-trigger (6)
/// block's spell; a first target of 5 (the pet) makes the pet the learner.
pub(crate) fn taught_spell(
    t: &ItemInfo,
    spells: Option<&crate::ui_action::Spells>,
) -> Option<(u32, bool)> {
    let block0 = t.spells.iter().find(|s| s.index == 0)?;
    let sd = spells?
        .catalog
        .get(block0.spell_id)
        .filter(|sd| sd.effects[0] == 36)?;
    let taught = match sd.effect_trigger_spell[0] {
        0 => t
            .spells
            .iter()
            .find(|s| s.trigger == 6)
            .map_or(0, |s| s.spell_id),
        id => id,
    };
    let pet = sd.effect_implicit_target_a[0] == 5 || sd.effect_implicit_target_b[0] == 5;
    Some((taught, pet))
}

/// Answers the engine's set-id asks from `ItemSet.dbc`: member names from the item-name cache, each
/// miss queried as the reference queries set members, and bonus text from the `$`-engine. A set
/// stays pending, re-pushing, until every member resolves.
pub(super) fn feed_item_sets(
    (script, _live): (Option<NonSendMut<UiScript>>, crate::ui_script::LendLive),
    sets: Option<Res<super::ItemSets>>,
    items: Res<Items>,
    // The pieces' names and slot types, the set block's own cache (`0x62ba33`).
    names: Res<crate::item_names::ItemNames>,
    commands: Res<NetCommands>,
    spells: Option<Res<crate::ui_action::Spells>>,
    spell_mods: Res<crate::spell::SpellModifiers>,
    skill_lines: Option<Res<crate::ui_spellbook::SkillLines>>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    caster_sources: crate::spell::text_caster::CasterSources,
    mut pending: Local<
        crate::ui_script::VmMemo<std::collections::HashMap<u32, benilla_ui::script::ItemSetView>>,
    >,
    mut mod_sensitive: Local<crate::ui_script::VmMemo<HashSet<u32>>>,
    mut last_reads: Local<crate::ui_script::VmMemo<Option<TokenReads>>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let pending = pending.get(&script);
    let mod_sensitive = mod_sensitive.get(&script);
    let me = self_q.single().ok();
    for id in script.take_item_set_asks() {
        pending.entry(id).or_default();
    }
    // The `$`-tokens follow the player's skills and gender.
    let reads = token_reads(me, &items);
    let gender = reads.1;
    let reads_changed = last_reads.get(&script).as_ref() != Some(&reads);
    let operands = reads.2.clone();
    *last_reads.get(&script) = Some(reads);
    if spell_mods.is_changed() || reads_changed || caster_sources.changed() {
        for &id in mod_sensitive.iter() {
            pending.entry(id).or_default();
        }
    }
    if pending.is_empty() {
        return;
    }
    let Some(sets) = sets.as_deref() else {
        return; // no catalog without client data: the asks stay parked
    };
    let spell_res = spells.as_deref();
    let skill_catalog = skill_lines.as_deref().map(|s| &*s.catalog);
    let skill = |id| crate::spell::spell_skill_value(me, skill_catalog, id);
    let cost = |d: &benilla_formats::SpellDisplay| {
        me.map_or(d.mana_cost, |s| {
            crate::spell::usable::power_cost(d, s, &spell_mods)
        }) as i32
    };
    let text_caster = me.map(|store| {
        crate::spell::text_caster::untracked(
            store,
            (caster_sources.known(), caster_sources.auras()),
            &operands,
            &spell_mods,
            &skill,
            &cost,
        )
    });
    let caster = text_caster
        .as_ref()
        .map(|c| c as &dyn benilla_formats::Caster);
    let mut done: Vec<u32> = Vec::new();
    let mut push: Vec<(u32, benilla_ui::script::ItemSetView)> = Vec::new();
    // Its borrow of the VM ends before the push, which needs the VM mutably.
    let global = |key: &str| benilla_ui::strings::global(script.lua(), key);
    for (&set_id, last) in pending.iter_mut() {
        let Some(row) = sets.0.set(set_id) else {
            done.push(set_id); // no such row: drop the ask
            continue;
        };
        if bonuses_have_mod_tokens(&row.bonuses, spell_res) {
            mod_sensitive.insert(set_id);
        } else {
            mod_sensitive.remove(&set_id);
        }
        let members: Vec<benilla_ui::script::ItemSetMember> = row
            .items
            .iter()
            .map(|&id| {
                let n = names.get_or_ask(id, &commands);
                benilla_ui::script::ItemSetMember {
                    id,
                    name: n.map(|n| n.name.clone()),
                    inventory_type: n.map_or(0, |n| n.inventory_type),
                }
            })
            .collect();
        let view = benilla_ui::script::ItemSetView {
            name: row.name.clone(),
            bonuses: row
                .bonuses
                .iter()
                .filter_map(|&(n, spell)| {
                    spell_desc_text(
                        spell_res,
                        spell,
                        &skill,
                        None,
                        &|| gender,
                        Some(&spell_mods),
                        &global,
                        caster,
                    )
                    .map(|desc| (n, desc))
                })
                .collect(),
            required_skill: row.required_skill,
            required_skill_rank: row.required_skill_rank,
            required_skill_name: (row.required_skill != 0)
                .then(|| {
                    skill_catalog
                        .and_then(|sl| sl.line(row.required_skill))
                        .map(|l| l.name.clone())
                })
                .flatten(),
            members,
        };
        // Done once every piece is answered; one the server does not know keeps no name.
        if row.items.iter().all(|&id| names.answered(id)) {
            done.push(set_id);
        }
        if *last != view {
            *last = view.clone();
            push.push((set_id, view));
        }
    }
    for (id, view) in push {
        script.set_item_set(id, view);
    }
    for id in done {
        pending.remove(&id);
    }
}

/// The client's loaded item tables in the shape the verbs read, built at load and read in place through the lent view at each call, as the reference reads its loaded stores.
#[derive(Resource, Default)]
pub(crate) struct ItemTablesLive {
    pub(crate) tables: benilla_ui::script::ItemTables,
}

/// Builds [`ItemTablesLive`] once the catalogs load: the roll tables need the enchant catalog (without it every row would build empty), the two scaling tables stand alone.
pub(super) fn build_item_tables(
    mut commands: Commands,
    props: Option<Res<crate::items::RandomProperties>>,
    enchants: Option<Res<crate::items::Enchants>>,
    scaling: Option<Res<super::ScalingStats>>,
) {
    let mut tables = benilla_ui::script::ItemTables::default();
    if let (Some(props), Some(enchants)) = (props, enchants) {
        build_random_properties(&mut tables, &props, &enchants);
    }
    if let Some(scaling) = scaling {
        tables.scaling = scaling_tables(&scaling);
    }
    commands.insert_resource(ItemTablesLive { tables });
}

fn build_random_properties(
    out: &mut benilla_ui::script::ItemTables,
    props: &crate::items::RandomProperties,
    enchants: &crate::items::Enchants,
) {
    let rows = crate::items::random_property_views(props, Some(enchants));
    let with_lines = rows.values().filter(|v| !v.enchants.is_empty()).count();
    info!(
        "random-property table: {} rows, {with_lines} with enchant lines",
        rows.len()
    );
    out.random_properties = rows;
    out.random_suffixes = props
        .0
        .iter_suffixes()
        .map(|(id, r)| {
            (
                id,
                benilla_ui::script::RandomSuffixView {
                    name: r.name.clone(),
                    enchants: r.enchants,
                    allocation: r.allocation,
                },
            )
        })
        .collect();
    out.enchant_rows = crate::items::enchant_rows(enchants);
}

fn scaling_tables(tables: &super::ScalingStats) -> benilla_ui::script::ScalingTables {
    benilla_ui::script::ScalingTables {
        distributions: tables
            .0
            .distributions
            .iter()
            .map(|(&id, d)| {
                (
                    id,
                    benilla_ui::script::ScalingDistributionView {
                        stat_ids: d.stat_ids,
                        bonuses: d.bonuses,
                        max_level: d.max_level,
                    },
                )
            })
            .collect(),
        values: tables
            .0
            .values
            .iter()
            .map(|&row| benilla_ui::script::ScalingValuesView(row))
            .collect(),
    }
}

/// The templates landed since [`feed_item_stats`] last pushed, moved out of the cache by
/// [`take_fresh_templates`], so the feed lending the VM only reads the cache.
#[derive(Resource, Default)]
pub(crate) struct FreshTemplates(Vec<u32>);

/// Move the landed templates to [`FreshTemplates`]; holds no VM.
pub(crate) fn take_fresh_templates(mut items: ResMut<Items>, mut fresh: ResMut<FreshTemplates>) {
    if items.has_fresh() {
        fresh.0.extend(items.take_fresh());
    }
}

/// The item-tooltip feed: every template that lands is pushed unprompted, so the first hover never
/// misses, as the reference reads one item cache synchronously; a read of an unresolved id records
/// a miss, which queries it here.
#[allow(clippy::type_complexity)]
pub(crate) fn feed_item_stats(
    (script, _live): (Option<NonSendMut<UiScript>>, crate::ui_script::LendLive),
    (items, mut fresh): (Res<Items>, ResMut<FreshTemplates>),
    commands: Res<NetCommands>,
    spells: Option<Res<crate::ui_action::Spells>>,
    // The caster the `$`-tokens read: its spell modifiers and its skills.
    (spell_mods, self_q, caster_sources): (
        Res<crate::spell::SpellModifiers>,
        Query<&ObjectStore, With<SelfPlayer>>,
        crate::spell::text_caster::CasterSources,
    ),
    skill_lines: Option<Res<crate::ui_spellbook::SkillLines>>,
    // The `$z` token: the bind point's area (`SMSG_BINDPOINTUPDATE`), named through `AreaTable`.
    home_bind: Option<Res<crate::net::HomeBind>>,
    area_names: Option<Res<crate::ui_quest_log::QuestHeaderNamesRes>>,
    factions: Option<Res<crate::target::Factions>>,
    sub_classes: Option<Res<super::ItemSubClasses>>,
    // `GetItemInfo`'s `itemType` and `itemTexture`.
    classes: Option<Res<super::ItemClasses>>,
    // The bag icons, the socket bonus's name and the limit category's quantity.
    (icons, enchants, limits): (
        Option<Res<ItemDisplays>>,
        Option<Res<crate::items::Enchants>>,
        Option<Res<crate::ui_trade::ItemLimitCategories>>,
    ),
    mut pending: Local<crate::ui_script::VmMemo<std::collections::HashSet<u32>>>,
    mut mod_sensitive: Local<crate::ui_script::VmMemo<ModSensitive>>,
    mut last_home: Local<crate::ui_script::VmMemo<Option<String>>>,
    mut last_seen: Local<crate::ui_script::VmMemo<Option<Seen>>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let pending = pending.get(&script);
    let mod_sensitive = mod_sensitive.get(&script);
    let last_home = last_home.get(&script);
    let me = self_q.single().ok();
    // The `$`-tokens follow the player's skills, gender, fields, spell book and auras.
    let inputs = Inputs {
        player: me.is_some(),
        reads: token_reads(me, &items),
        cast_speed: me.and_then(|s| s.0.unit_mod_cast_speed()).unwrap_or(1.0),
        known: caster_sources.known(),
        known_written: caster_sources.known_changed(),
        auras: caster_sources.auras(),
        auras_written: caster_sources.auras_changed(),
    };
    // Every `$` view when an input no view records moved; else the views that read what moved.
    let (all_due, moved, seen) = match last_seen.get(&script) {
        Some(seen) => {
            let (all_due, moved) = seen.advance(inputs);
            (all_due || spell_mods.is_changed(), moved, &*seen)
        }
        none => (true, Moved::default(), &*none.insert(Seen::new(inputs))),
    };
    let (gender, operands) = (seen.gender(), seen.operands());
    // `GetBindLocation()`'s push, here so it and the `$z` token share one name, and ahead of the
    // pending gate below: the bind point can arrive while the feed idles.
    let home_area: Option<&str> = home_bind
        .as_deref()
        .and_then(|b| b.0)
        .and_then(|id| area_names.as_deref()?.0.resolve(id as i32));
    // Pushed on change, per VM: a rebuilt VM is fed again the frame it appears.
    if last_home.as_deref() != home_area {
        script.set_bind_location(home_area.unwrap_or_default());
        *last_home = home_area.map(str::to_string);
        // Re-substitute every held view, or one pushed before the bind point carries a raw `$z`.
        pending.extend(items.cached_template_ids());
    }

    if !fresh.0.is_empty() {
        pending.extend(fresh.0.drain(..));
    }
    pending.extend(script.take_item_stat_asks());
    if all_due {
        pending.extend(mod_sensitive.all.iter().copied());
    } else if moved.any {
        pending.extend(
            mod_sensitive
                .reads
                .iter()
                .filter(|(_, reads)| reads.hits(&moved))
                .map(|(&id, _)| id),
        );
    }
    if pending.is_empty() {
        return;
    }
    let spell_res = spells.as_deref();
    let skill_catalog = skill_lines.as_deref().map(|s| &*s.catalog);
    let skill = |id| crate::spell::spell_skill_value(me, skill_catalog, id);
    // What the view being built reads of the player, taken as each view is done.
    let view_reads = std::cell::RefCell::new(ViewReads::default());
    let cost = |d: &benilla_formats::SpellDisplay| {
        view_reads.borrow_mut().power_cost();
        me.map_or(d.mana_cost, |s| {
            crate::spell::usable::power_cost(d, s, &spell_mods)
        }) as i32
    };
    let record = |read| view_reads.borrow_mut().caster_read(read);
    let text_caster = me.map(|store| crate::spell::text_caster::TextCaster {
        store,
        known: caster_sources.known(),
        auras: caster_sources.auras(),
        operands,
        mods: &spell_mods,
        skill: &skill,
        power_cost: &cost,
        on_read: &record,
    });
    let caster = text_caster
        .as_ref()
        .map(|c| c as &dyn benilla_formats::Caster);
    let ready: Vec<u32> = pending
        .iter()
        .copied()
        .filter(|&id| items.template(id, 0, &commands).is_some())
        .collect();
    // Built first, pushed after: the lookup borrows the VM that `set_item_template` needs mutably.
    let views: Vec<(u32, benilla_ui::script::ItemTemplateView)> = {
        let get = |key: &str| {
            script
                .lua()
                .globals()
                .get::<String>(key)
                .ok()
                .filter(|t| !t.is_empty())
        };
        ready
            .into_iter()
            .filter_map(|id| {
                pending.remove(&id);
                let t = items.template(id, 0, &commands)?.clone();
                let view = template_view(
                    &t,
                    spell_res,
                    Some(&spell_mods),
                    &skill,
                    skill_catalog,
                    home_area,
                    &|| gender,
                    factions.as_deref().map(|f| f.catalog()),
                    sub_classes.as_deref().map(|s| &s.0),
                    classes.as_deref().map(|c| &c.0),
                    icons.as_deref(),
                    &get,
                    caster,
                    enchants.as_deref(),
                    limits.as_deref(),
                );
                mod_sensitive.hold(id, item_has_mod_tokens(&t, spell_res), view_reads.take());
                Some((id, view))
            })
            .collect()
    };
    for (id, view) in views {
        script.set_item_template(id, view);
    }
}

/// The player state the tooltip's red lines and the usable gate check, built from the live state at the call: level, class, race, each skill with its permanent bonus, the proficiency masks (`SMSG_SET_PROFICIENCY`), each faction's rank (the DBC base plus the standing), the dual-wield test, the honor rank, the medals and the class flags (`0x6288f5`).
pub(crate) fn player_req(
    me: &benilla_protocol::ObjectFields,
    proficiencies: Option<&crate::net::Proficiencies>,
    reputations: Option<&crate::net::Reputations>,
    factions: Option<&crate::target::Factions>,
    actions: Option<&crate::ui_action::PlayerActions>,
    spells: Option<&crate::ui_action::Spells>,
    classes: Option<&crate::chr_classes::ChrClassTable>,
) -> benilla_ui::script::PlayerReqState {
    let mut skills = std::collections::HashMap::new();
    for slot in 0..benilla_protocol::messages::PLAYER_SKILL_SLOTS {
        let Some(s) = me.player_skill(slot) else {
            continue;
        };
        if s.skill_id != 0 {
            // The value plus the permanent bonus, never the temporary one.
            skills.insert(
                u32::from(s.skill_id),
                u32::from(s.value).saturating_add_signed(i32::from(s.perm_bonus)),
            );
        }
    }
    let race = me.unit_race().unwrap_or(0);
    let class = me.unit_class().unwrap_or(0);
    let mut rep_ranks = std::collections::HashMap::new();
    if let Some(cat) = factions.map(|f| f.catalog()) {
        for (id, info) in cat.reputation_factions() {
            let standing = usize::try_from(info.rep_index)
                .ok()
                .and_then(|i| reputations?.0.get(i))
                .map(|&(_, s)| s)
                .unwrap_or(0);
            rep_ranks.insert(
                id,
                benilla_formats::reputation_rank(info.base_for(race, class) + standing),
            );
        }
    }
    // The dual-wield test: a known spell whose Effect[0] is 40 (`SPELL_EFFECT_DUAL_WIELD`).
    let can_dual_wield = spells.zip(actions).is_some_and(|(s, actions)| {
        actions
            .spells
            .iter()
            .any(|&id| s.catalog.get(id).is_some_and(|sd| sd.effects[0] == 40))
    });
    benilla_ui::script::PlayerReqState {
        level: me.unit_level().unwrap_or(0),
        class_id: u32::from(class),
        race_id: u32::from(race),
        skills,
        proficiency: proficiencies.map(|p| p.0.clone()).unwrap_or_default(),
        rep_ranks,
        can_dual_wield,
        honor_rank: me.player_honor_rank().unwrap_or(0),
        pvp_medals: me.player_pvp_medals().unwrap_or(0),
        class_flags: classes.map_or(0, |c| c.0.flags(u32::from(class))),
    }
}

/// One bag slot from its item guid: instance, template, icon and use-spell cooldown. `None` is an
/// empty slot; an occupied one not yet resolved is `Some` with empty fields.
pub(super) fn resolve_slot(
    guid: u64,
    objects: &Objects,
    items: &Items,
    icons: Option<&ItemDisplays>,
    rolls: crate::items::RollCatalogs,
    commands: &NetCommands,
    cooldowns: Option<&crate::spell::Cooldowns>,
    spells: Option<&benilla_formats::SpellCatalog>,
    names: &crate::names::NameCache,
    // Reading a charter's record is what queries it.
    petitions: Option<&crate::ui_petition::PetitionState>,
    now: std::time::Instant,
    ui_now: f64,
    // The class's `ChrClasses.dbc` relic flag, which decides slot 17 for every fit-rule read.
    has_relic_slot: bool,
) -> Option<ContainerSlot> {
    if guid == 0 {
        return None;
    }
    // The enchant and lifetime (`SMSG_ITEM_TIME_UPDATE`) countdowns, in whole seconds.
    let countdowns = objects.countdowns(guid);
    let enchant_ms: [Option<u64>; crate::items::ENCHANT_SLOTS] =
        std::array::from_fn(|s| countdowns.and_then(|c| c.enchant_remaining_display_ms(s as u32)));
    let duration_ms = countdowns.and_then(|c| c.lifetime_remaining_display_ms());
    let gems = objects
        .object(guid)
        .map_or([0; 3], |f| crate::items::gems(f, rolls.enchants));
    let (entry, count, durability, readable, creator, flags, already_bound, roll, enchant_lines) =
        match objects.object(guid) {
            Some(fields) => (
                fields.object_entry().unwrap_or(0),
                fields.item_stack_count().unwrap_or(1),
                // Max 0 is indestructible: no line.
                fields
                    .item_durability()
                    .zip(fields.item_max_durability())
                    .filter(|&(_, max)| max > 0),
                // Letter text on the instance, which the hover magnifier keys off; a template
                // `PageText` book does not set it.
                fields.item_has_text(),
                // The creator's name for "<Made by %s>" and "Written by %s", `None` until the
                // name query answers.
                fields
                    .item_creator()
                    .filter(|&g| g != 0)
                    .and_then(|g| names.resolve(g, commands).map(str::to_string)),
                // The tooltip's unlocked (0x4) and wrapped (0x8) bits.
                fields.item_flags().unwrap_or(0),
                // Soulbound, or carrying a binding enchant, off the raw descriptor.
                crate::items::already_bound(fields, rolls.enchants),
                // The roll behind the name's suffix; its enchants are already in slots 7..11.
                fields.item_random_properties_id(),
                // All twelve enchant slots with charges and countdowns: our own items stream whole,
                // where others' carry two slots.
                crate::items::enchant_lines(
                    (0..crate::items::ENCHANT_SLOTS as u8).map(|s| {
                        (
                            s,
                            fields.item_enchant(s).unwrap_or(0),
                            fields.item_enchant_charges(s),
                            enchant_ms[usize::from(s)],
                        )
                    }),
                    rolls.enchants,
                ),
            ),
            // A guid whose create has not landed: occupied, unresolved.
            None => {
                return Some(ContainerSlot {
                    guid,
                    texture: Some(FALLBACK_ITEM_ICON.to_string()),
                    ..Default::default()
                })
            }
        };
    if entry == 0 {
        return Some(ContainerSlot {
            guid,
            texture: Some(FALLBACK_ITEM_ICON.to_string()),
            ..Default::default()
        });
    }
    // The bag icon needs no template (`0x5d7b18`).
    let icon = icons.map(|i| i.carried_icon(entry));
    let template: Option<ItemInfo> = items.template(entry, guid, commands).cloned();
    let Some(t) = template else {
        // Asked (or a cached negative); show the slot occupied while the answer is in flight.
        return Some(ContainerSlot {
            guid,
            gems,
            durability: None,
            texture: icon.or_else(|| Some(FALLBACK_ITEM_ICON.to_string())),
            item_id: entry,
            count,
            readable,
            creator,
            flags,
            already_bound,
            enchants: enchant_lines,
            duration_ms,
            ..Default::default()
        });
    };
    let link_fields = objects
        .object(guid)
        .map_or_else(Default::default, |o| super::object_link_fields(o, t.flags));
    Some(ContainerSlot {
        guid,
        gems,
        // `GetContainerItemQuestInfo`: bonding 4 is the quest-item border, `StartQuest` the bang.
        quest_item: t.bonding == 4,
        start_quest: t.start_quest,
        // A charter's guild name and master, keyed by the petition id the server puts in
        // enchantment slot 0 (`PetitionsHandler.cpp:126`), where the reference reads it and its
        // enchant lines skip it. The first hover sends the query.
        petition: (t.flags & benilla_protocol::messages::ITEM_FLAG_CHARTER != 0)
            .then(|| {
                let id = objects
                    .object(guid)
                    .and_then(|f| f.item_enchant(0))
                    .unwrap_or(0);
                petitions?.tooltip_view(id as u32, guid, names, commands)
            })
            .flatten(),
        texture: icon,
        count,
        durability,
        quality: Some(t.quality),
        item_id: entry,
        // The link carries the roll in `randomPropertyId` and in the suffixed name; the tooltip
        // reads its name back off it.
        link: Some(crate::ui_items::item_link_full(
            entry,
            &link_fields,
            &rolls.name(&t.name, roll),
            t.quality,
        )),
        locked: false,
        readable,
        creator,
        flags,
        already_bound,
        enchants: enchant_lines,
        duration_ms,
        equip_slots: find_equip_slot(t.inventory_type, has_relic_slot),
        bar_placeable: t.placeable_on_action_bar(),
        cooldown: t.use_spell.and_then(|u| {
            let sd = spells.and_then(|s| s.get(u.spell_id));
            cooldowns?
                .info(u.spell_id, entry, sd, now)
                .ui_triple(now, ui_now)
        }),
    })
}

/// Reason 16's `%s`, the target bag's `BagFamily` name ("Only Arrows can be placed in that."), or
/// `None` to keep the generic line. `bag_slot` is the bag's player-array slot, and the reference's
/// names nothing for 255 (the player's own array) or for a slot at or past `[player+0x1d38]`, 113
/// for the local player, so equipped bags (19..22) and bank bags (67..73) both resolve. It reads
/// the bag family, not the subclass the errorId's name suggests.
pub(super) fn bag_family_name(
    player: Option<&ObjectStore>,
    bag_slot: u8,
    objects: &Objects,
    items: &Items,
    families: Option<&benilla_formats::ItemBagFamilyCatalog>,
    commands: &NetCommands,
) -> Option<String> {
    let store = player?;
    let families = families?;
    let guid = match bag_slot {
        s if (BAG_SLOT_FIRST..BAG_SLOT_FIRST + BAGS).contains(&s) => store.0.player_inv_slot(s),
        s if (BANK_BAG_SLOT_FIRST..BANK_BAG_SLOT_FIRST + BANK_BAGS).contains(&s) => {
            store.0.player_bank_bag_slot(s - BANK_BAG_SLOT_FIRST)
        }
        // 255, the player's own array, names no bag.
        _ => None,
    }
    .filter(|&g| g != 0)?;
    let entry = objects.object(guid)?.object_entry().filter(|&e| e != 0)?;
    let family = items.template(entry, guid, commands)?.bag_family;
    families.name(family).map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::charges_count;
    use crate::ui_action::real_spells;
    use benilla_protocol::messages::ItemSpellEntry;

    fn slot(spell_id: u32, charges: i32) -> ItemSpellEntry {
        ItemSpellEntry {
            index: 0,
            spell_id,
            trigger: 0,
            charges,
            cooldown_ms: -1,
            category: 0,
            category_cooldown_ms: -1,
        }
    }

    #[test]
    fn modifier_refresh_tracks_only_item_and_set_descriptions_with_tokens() {
        let displays = std::collections::HashMap::from([
            (
                1,
                benilla_formats::SpellDisplay {
                    description: Some("Restores 5 health.".into()),
                    ..Default::default()
                },
            ),
            (
                2,
                benilla_formats::SpellDisplay {
                    description: Some("Restores $s1 health.".into()),
                    ..Default::default()
                },
            ),
        ]);
        let spells = crate::ui_action::Spells {
            power_displays: Default::default(),
            difficulties: Default::default(),
            instance_difficulty: Default::default(),
            catalog: benilla_formats::SpellCatalog::from_displays(displays),
            forms: Default::default(),
            ranges: Default::default(),
            cast_times: Default::default(),
            durations: Default::default(),
            radii: Default::default(),
        };
        let mut item = crate::items::test_template("Token item");
        item.spells = vec![slot(1, 0)];
        assert!(!super::item_has_mod_tokens(&item, Some(&spells)));
        item.spells.push(slot(2, 0));
        assert!(super::item_has_mod_tokens(&item, Some(&spells)));
        assert!(!super::bonuses_have_mod_tokens(&[(2, 1)], Some(&spells)));
        assert!(super::bonuses_have_mod_tokens(
            &[(2, 1), (4, 2)],
            Some(&spells)
        ));
    }

    /// The builder's charge gate.
    #[test]
    fn charge_gate_matches_the_real_builder() {
        // Food and water carry `-1` in vmangos `item_template` (bread 4540, water 159 and 5350).
        assert_eq!(charges_count(&[slot(433, -1)]), 0, "food's -1 = no line");
        // A pool that uses the item up: Flame Deflector 4376 (`4057, -5`).
        assert_eq!(charges_count(&[slot(4057, -5)]), 5, "wand-style pool");
        assert_eq!(charges_count(&[slot(4057, 3)]), 3);
        assert_eq!(charges_count(&[slot(433, 0)]), 0, "template 0 = sentinel");
        assert_eq!(charges_count(&[slot(0, -5)]), 0, "no spell = no slot");
        assert_eq!(charges_count(&[]), 0);
        // A leading sentinel slot does not mask a later pool.
        assert_eq!(charges_count(&[slot(433, -1), slot(4057, -10)]), 10);
    }

    /// On the real Spell.dbc, an undescribed spell such as a key's `Opening` builds no trigger
    /// line, and a described one does.
    #[test]
    fn an_undescribed_spell_prints_no_trigger_line_on_real_data() {
        let Some(spells) = real_spells() else { return };

        // No string table: Fireball's description reaches no keyed token.
        let no_strings = |_: &str| None;

        // The lock chain's Opening and Closing spells, none with a description.
        for id in [3365u32, 3366, 6246, 6247, 6477, 21651] {
            let d = spells.catalog.get(id).expect("a real Spell.dbc row");
            assert!(
                !d.name.is_empty(),
                "spell {id} has a name — which is exactly what must NOT leak into the tooltip"
            );
            assert_eq!(
                super::spell_desc_text(
                    Some(&spells),
                    id,
                    &|_| 0,
                    None,
                    &|| 0,
                    None,
                    &no_strings,
                    None
                ),
                None,
                "spell {id} ({:?}) has no description, so the reference prints no trigger line",
                d.name
            );
        }

        // The control: Fireball (133), described, still yields its line.
        let fireball = super::spell_desc_text(
            Some(&spells),
            133,
            &|_| 0,
            None,
            &|| 0,
            None,
            &no_strings,
            None,
        )
        .expect("a described spell still yields its line");
        assert!(
            fireball.contains("damage"),
            "expected the substituted Fireball description, got {fireball:?}"
        );
    }

    /// The trigger text's `$g` branch reads the gender handed in: Conjure Food 587's "providing the
    /// mage and `$ghis:her;` allies" takes "her" on 1, "his" on 0.
    #[test]
    #[ignore = "reads 1.12.1 data; port to the 3.3.5a install (#51)"]
    fn a_trigger_texts_gender_branch_follows_the_player() {
        let Some(spells) = real_spells() else { return };
        let no_strings = |_: &str| None;
        let line = |gender: u8| {
            super::spell_desc_text(
                Some(&spells),
                587,
                &|_| 0,
                None,
                &move || gender,
                None,
                &no_strings,
                None,
            )
            .expect("a described spell")
        };
        assert!(line(1).contains("the mage and her allies"), "{}", line(1));
        assert!(line(0).contains("the mage and his allies"), "{}", line(0));
    }

    /// The item feeds re-substitute a held `$`-text when what the tokens read moves: the gender
    /// byte `$g`/`$G` branch on, and no other byte of `UNIT_FIELD_BYTES_0`.
    #[test]
    fn the_token_reads_move_with_the_gender_byte_alone() {
        let store = |bytes: u32| {
            crate::net::ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[(
                benilla_protocol::field::FIELD_UNIT_BYTES_0,
                bytes,
            )]))
        };
        let items = crate::items::Items::default();
        let male = super::token_reads(Some(&store(4 | 1 << 8)), &items);
        assert_ne!(
            super::token_reads(Some(&store(4 | 1 << 8 | 1 << 16)), &items),
            male
        );
        assert_eq!(
            super::token_reads(Some(&store(1 | 2 << 8 | 1 << 24)), &items),
            male
        );
    }
}
