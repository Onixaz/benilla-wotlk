//! The item tooltip's line law, [`render_view`]: the emission order of the reference's builder.
//! Every sentence is a key the builder itself names (the string pointers reachable), never a match
//! on English: `ITEM_REQ_SKILL` and the three `LOCKED_WITH_*` keys all read "Requires %s" in enUS
//! and differ elsewhere. A key the player's `GlobalStrings.lua` lacks emits no line, as in the
//! reference, whose global-string lookup returns the empty string and whose `AddLine` drops an
//! empty row uncounted.

use mlua::{Lua, Table};

use crate::script::tooltip::{append_line, duration_text, plural_template};
use crate::script::ItemTemplateView;
use crate::strings::{fill, Arg};

use super::names::*;
use super::set_count::{count_set, WornPiece};
use super::sockets::{empty_socket, socket_lines};

/// What a source knows beyond the template: an item object's fields, or a block source's name and
/// roll. A template source passes `None`.
#[derive(Default)]
pub(super) struct ItemInstance {
    /// The name with its random suffix joined, which the reference composes in the builder (through
    /// `ITEM_SUFFIX_TEMPLATE`); `None` takes the template's.
    pub name: Option<String>,
    /// Live `(current, max)`; `None` for an indestructible item or one not yet streamed.
    pub durability: Option<(u32, u32)>,
    /// The resolved `ITEM_FIELD_CREATOR` name; `None` while it is unresolved, which prints no line
    /// either.
    pub creator: Option<String>,
    /// `ITEM_FIELD_FLAGS & 0x200`, a letter's copy: the creator line reads WRITTEN_BY and READABLE
    /// shows.
    pub has_text: bool,
    /// `ITEM_FIELD_FLAGS`: UNLOCKED `0x4`, WRAPPED `0x8`.
    pub flags: u32,
    /// Runtime-bound: `ITEM_FIELD_FLAGS & 1`, or an enchant that binds. App-resolved, since
    /// [`Self::enchants`] drops rows the line law hides.
    pub already_bound: bool,
    /// The petition a charter names, printed between the name and `ITEM_SIGNABLE`.
    pub petition: Option<crate::script::PetitionSlotView>,
    /// The enchant slots, app-resolved, in slot order.
    pub enchants: Vec<crate::script::EnchantView>,
    /// The reference's p6 = 0, no caller-supplied instance block (`[this+0x440]`, when set, only
    /// READABLE is evaluated). It is per call site, not per binding: `SetBagItem` takes p6=1 iff
    /// the cooldown query finds one running, else p6=0, so the open line and the cooldown line
    /// never show together there.
    pub openable_source: bool,
    /// A timed item's remaining lifetime in ms, from `SMSG_ITEM_TIME_UPDATE`, not
    /// `ITEM_FIELD_DURATION`: the client displays from the packet (`Objects/Item.cpp:1093`).
    pub duration_ms: Option<u64>,
    /// The item object's guid, which the equipment-set line looks up; 0 for a source with none.
    pub guid: u64,
    /// p9, the socket preview, `SetSocketedItem` alone: the gem placed per socket, which stands in its
    /// socket (`0x6277f0`). The preview cuts the bind region, a gem's own line, durability, the
    /// class, race and requirement lines, the set block and the creator line.
    pub socket_preview: Option<[Option<crate::script::ItemTemplateView>; 3]>,
}

/// The sets holding `item` (`0x5ae380`): each set's name once per slot that holds it, in set
/// order, joined by `delimiter`; `None` when no set does.
pub(super) fn equipment_set_list(
    sets: &[crate::script::EquipmentSetView],
    item: u64,
    delimiter: &str,
) -> Option<String> {
    let mut out = String::new();
    for set in sets {
        for _ in set.items.iter().filter(|&&g| g == item) {
            if !out.is_empty() {
                out.push_str(delimiter);
            }
            out.push_str(&set.name);
        }
    }
    (!out.is_empty()).then_some(out)
}

/// The set block's blank gold line, the reference's literal: a space and a newline. An empty string
/// would be no row, as `AddLine` drops an empty left text uncounted. `" \n"` is one row: the
/// stepper consumes the break with the space, as the app's `fontstring_lines` trailing-break rule
/// does.
const SET_SPACER: &str = " \n";

/// Where the money block (`0x62d71b`) takes its stack from: the builder's item object or its
/// link-info block, or nowhere when the binding passes `skipSellPrice` (`0x6277f0`).
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub(in crate::script) enum Sell {
    /// `skipSellPrice` set, as `SetMerchantItem`, `SetAuctionItem` and `SetTrainerService` pass.
    #[default]
    Skip,
    /// An item object: SellPrice less its repair cost (1 when the repair costs as much), times the
    /// stack. `worn` is a player slot below 23, where a price of 0 prints no ITEM_UNSELLABLE.
    Object {
        stack: u32,
        repair_cost: u32,
        worn: bool,
    },
    /// No object: SellPrice × the block's stack, with a maxcost when `max` exceeds the stack.
    Block { stack: u32, max: Option<u32> },
}

impl Sell {
    /// A template with no link info, the `stackCount` 1 the link info's clear leaves.
    pub(in crate::script) const ONE: Self = Self::Block {
        stack: 1,
        max: None,
    };
}

/// The builder's render flags: p4 and p5, and the money source.
#[derive(Clone, Copy, Default)]
pub(super) struct BuilderFlags {
    /// p5 `[arg+0x18]`: prepend the gray `CURRENTLY_EQUIPPED` line. Only the two compare bindings
    /// set it, and both pass [`name_only`](Self::name_only) zero.
    pub currently_equipped: bool,
    /// p4 `[arg+0x14]`, the compact mode the reference calls `nameOnly`, reachable only through
    /// `SetInventoryItem`'s third argument. The name goes white, the bind/lock region and the stat
    /// body are cut, and all after the cooldown line is skipped; the slot/type cell, durability,
    /// duration, requirements, spell triggers and set block still print.
    pub name_only: bool,
    pub sell: Sell,
}

/// Render one item into the tooltip's order. Not built: ITEM_COOLDOWN_TIME, ITEM_WRAPPED_BY and the
/// `LOCKED_WITH_*` line under LOCKED.
pub(super) fn render_view(
    lua: &Lua,
    this: &Table,
    v: &ItemTemplateView,
    flags: BuilderFlags,
    // `None`: a template source, with no item object and no instance block.
    inst: Option<&ItemInstance>,
) -> mlua::Result<()> {
    let (req, known_spell, taught_known, set_view, worn, show_item_level) = {
        let mut model = crate::script::model_mut_of(lua);
        let show_item_level = model
            .cvars
            .get("showitemlevel")
            .is_some_and(|c| c.value.trim().parse::<f64>().is_ok_and(|n| n != 0.0));
        // The known bits (the unit's spell-known test, `0x7260e0`): a mount or companion counts.
        let knows = |id: u32| model.spellbook.named().any(|s| s.spell_id == id);
        let known = v.required_spell != 0 && knows(v.required_spell);
        // A taught spell (trigger 6, learn) the player knows: the always-red ITEM_SPELL_KNOWN.
        let taught = v
            .spell_triggers
            .iter()
            .any(|&(t, id, _)| t == 6 && id != 0 && knows(id));
        // The set view (a miss records the ask) and the equipped ids the set block counts.
        let set_view = (v.item_set != 0)
            .then(|| {
                let view = model.item_sets.get(&v.item_set).cloned();
                if view.is_none() {
                    model.item_set_asks.insert(v.item_set);
                    super::note_set_miss(&mut model, v.item_set);
                }
                view
            })
            .flatten();
        // The worn items' templates (slots 0..=18), which the set block counts by their own set id;
        // one still in flight is asked for and counts once it lands.
        let worn: Vec<WornPiece> = match &set_view {
            Some(_) => {
                let ids: Vec<u32> = model
                    .inventory_slots
                    .iter()
                    .take(19)
                    .flatten()
                    .map(|s| s.item_id)
                    .filter(|&id| id != 0)
                    .collect();
                ids.into_iter()
                    .filter_map(|id| match model.item_templates.get(&id) {
                        Some(t) => Some(WornPiece {
                            id,
                            name: t.name.clone(),
                            item_set: t.item_set,
                            inventory_type: t.inventory_type,
                        }),
                        None => {
                            model.item_stat_asks.insert(id);
                            super::note_item_miss(&mut model, id);
                            None
                        }
                    })
                    .collect()
            }
            None => Vec::new(),
        };
        (
            model.player_req.clone(),
            known,
            taught,
            set_view,
            worn,
            show_item_level,
        )
    };
    let add = |l: (String, [f32; 4])| append_line(lua, this, l, None, false);
    let addw = |l: (String, [f32; 4])| append_line(lua, this, l, None, true);
    let add2 =
        |l: (String, [f32; 4]), r: (String, [f32; 4])| append_line(lua, this, l, Some(r), false);
    let get = |key: &str| crate::strings::global(lua, key);
    let keyed = |key: &str, args: &[Arg<'_>], color: [f32; 4], wrap: bool| -> mlua::Result<()> {
        match get(key) {
            Some(t) => append_line(lua, this, (fill(&t, args), color), None, wrap),
            None => Ok(()),
        }
    };

    // The header (`[arg+0x18]`) is all a compare adds: the only call sites passing p5 pass p4 = 0,
    // so none is cut. For a gem it is the red DESTROY_GEM, which grays the name and the gem line
    // (`SetExistingSocketGem`'s `toDestroy`).
    let destroy_gem = flags.currently_equipped && v.gem_properties != 0;
    if destroy_gem {
        keyed("DESTROY_GEM", &[], RED, false)?;
    } else if flags.currently_equipped {
        keyed("CURRENTLY_EQUIPPED", &[], GRAY, false)?;
    }
    let preview = inst.is_some_and(|i| i.socket_preview.is_some());
    let name = inst
        .and_then(|i| i.name.clone())
        .unwrap_or_else(|| v.name.clone());
    // `nameOnly` paints the name white, else the quality arm colours it; nothing else recolors it,
    // not even for an unusable item.
    let name_color = if flags.name_only {
        WHITE
    } else if destroy_gem {
        GRAY
    } else {
        quality_color(v.quality)
    };
    add((name, name_color))?;
    // The petition lines, below the name: a charter's `GUILD_CHARTER_*` pair or a petition's
    // `PETITION_*`, by the record's charter bit (`GetPetitionInfo`'s first return). A line whose
    // source is unresolved waits for the repaint, as the creator line does.
    if let Some(p) = inst.and_then(|i| i.petition.as_ref()) {
        let (title_key, creator_key) = if p.is_charter {
            ("GUILD_CHARTER_TITLE", "GUILD_CHARTER_CREATOR")
        } else {
            ("PETITION_TITLE", "PETITION_CREATOR")
        };
        if !p.title.is_empty() {
            keyed(title_key, &[Arg::S(&p.title)], WHITE, false)?;
        }
        if let Some(owner) = p.owner.as_deref().filter(|o| !o.is_empty()) {
            keyed(creator_key, &[Arg::S(owner)], WHITE, false)?;
        }
    }
    // ITEM_SIGNABLE: template flag `0x2000`, a petition.
    if v.flags & 0x2000 != 0 {
        keyed("ITEM_SIGNABLE", &[], GREEN, false)?;
    }
    // p4's first cut: the bind/lock region, which the socket preview cuts too. ITEM_SIGNABLE above
    // survives it.
    if !flags.name_only && !preview {
        if v.flags & 0x2 != 0 {
            keyed("ITEM_CONJURED", &[], WHITE, false)?;
        }
        // Bonding (`[record+0x194]`) 1..5 decides whether a bind line prints. A runtime-bound
        // instance prints ITEM_SOULBOUND, or ITEM_BIND_QUEST for the quest kinds; otherwise the
        // jump table picks 1 pickup, 2 equip, 3 use, 4 and 5 quest.
        match v.bonding {
            4 | 5 => keyed("ITEM_BIND_QUEST", &[], WHITE, false)?,
            1..=3 if inst.is_some_and(|i| i.already_bound) => {
                keyed("ITEM_SOULBOUND", &[], WHITE, false)?
            }
            1 => keyed("ITEM_BIND_ON_PICKUP", &[], WHITE, false)?,
            2 => keyed("ITEM_BIND_ON_EQUIP", &[], WHITE, false)?,
            3 => keyed("ITEM_BIND_ON_USE", &[], WHITE, false)?,
            _ => {}
        }
        match v.max_count {
            1 => keyed("ITEM_UNIQUE", &[], WHITE, false)?,
            n if n > 1 => keyed("ITEM_UNIQUE_MULTIPLE", &[Arg::D(n.into())], WHITE, false)?,
            _ => {}
        }
        if v.start_quest != 0 {
            keyed("ITEM_STARTS_QUEST", &[], WHITE, false)?;
        }
        // LOCKED, red, until the instance carries UNLOCKED `0x4`.
        if v.lock_id != 0 && inst.is_none_or(|i| i.flags & 0x4 == 0) {
            keyed("LOCKED", &[], RED, false)?;
        }
    }
    // The slot | type line, or for a bag the CONTAINER_SLOTS line in its seat. The type cell is
    // hidden for cloaks (16) and display-hidden subclasses. The cells recolor independently: a
    // proficiency miss (`[class]` bit `1 << subclass`; a class with no mask never reds) reds the
    // type, or the slot when a weapon's alternate subclass is proficient; an off-hand weapon (22)
    // without Dual Wield reds the slot. The bag gate is `InventoryType == 18` alone, never the slot
    // count. Quivers and ammo pouches are 18 too, so 27 is dead (maps to no slot).
    if v.inventory_type == 18 {
        // `%s` is the subclass DisplayName the type cell reads ("8 Slot Quiver"); a subclass
        // without one prints no line at all, and displayFlags bit 0 is not read here.
        if let Some(name) = v.sub_class_display.as_deref() {
            keyed(
                "CONTAINER_SLOTS",
                &[Arg::D(v.container_slots.into()), Arg::S(name)],
                WHITE,
                false,
            )?;
        }
    } else {
        // The slot cell: for `ItemClass == 6`, `ItemClass.dbc`'s own name ("Projectile"), the
        // column `GetItemInfo`'s itemType reads; else the key, so a bow reads "Ranged | Bow" and a
        // gun, whose key ships no string, "Gun" alone.
        let slot = if v.class == 6 {
            v.item_type.clone()
        } else {
            invtype_key(v.inventory_type).and_then(&get)
        };
        // The type cell: `ItemSubClass.dbc`'s DisplayName, app-resolved.
        let ty = if v.inventory_type == 16 || v.hide_subclass {
            None
        } else {
            v.sub_class_display.as_deref()
        };
        let mut left_red = false;
        let mut right_red = false;
        if let Some(&mask) = req.proficiency.get(&v.class) {
            if mask & (1 << v.subclass) == 0 {
                let alt_ok =
                    v.class == 2 && v.proficiency_alt.is_some_and(|a| mask & (1 << a) != 0);
                if alt_ok {
                    left_red = true;
                } else {
                    right_red = true;
                }
            }
        }
        if v.inventory_type == 22 && !req.can_dual_wield {
            left_red = true;
        }
        match (slot, ty) {
            (Some(s), Some(t)) => add2(
                (s, req_color(!left_red)),
                (t.to_string(), req_color(!right_red)),
            )?,
            (Some(s), None) => add((s, req_color(!left_red)))?,
            // No slot name: the type stands alone, keeping its own colour.
            (None, Some(t)) => add((t.to_string(), req_color(!right_red)))?,
            _ => {}
        }
    }
    // p4's second cut: damage through the enchant family. The slot/type cell between the two cuts
    // always prints.
    let mut feral_ap: Option<i64> = None;
    if !flags.name_only {
        // The damage block: per slot, a key picked by school (non-zero), ammo (`ItemClass == 6`
        // alone) and, on the no-school non-ammo leaf only, single (`floor(min) == ceil(max)`);
        // every emitted slot after the first takes `PLUS_`.
        let mut first = true;
        let mut dps_acc = 0.0f32;
        for &(min, max, school) in v.damages.iter().take(5) {
            // The biased floor and ceil, not rounding: 38.7-85.7 reads "38 - 86".
            let (lo, hi) = (floor_min(min), ceil_max(max));
            // A slot is emitted iff either rounded bound is nonzero.
            if lo == 0 && hi == 0 {
                continue;
            }
            // The school number picks the arm; a missing `SPELL_SCHOOL%d_CAP` fills its hole empty.
            let school_name = school_key(school).and_then(|k| get(&k)).unwrap_or_default();
            // The rounded bounds' mean, not per second whatever `AMMO_DAMAGE_TEMPLATE` says: an
            // arrow of 1-2 reads "Adds 1.5 damage per second".
            let avg = f64::from((lo + hi) as f32 * 0.5);
            let plus = |k: &str| {
                if first {
                    k.to_string()
                } else {
                    format!("PLUS_{k}")
                }
            };
            let (key, args): (String, Vec<Arg<'_>>) = if school != 0 {
                if v.class == 6 {
                    (
                        plus("AMMO_SCHOOL_DAMAGE_TEMPLATE"),
                        vec![Arg::F(avg), Arg::S(&school_name)],
                    )
                } else {
                    (
                        plus("DAMAGE_TEMPLATE_WITH_SCHOOL"),
                        vec![Arg::D(lo.into()), Arg::D(hi.into()), Arg::S(&school_name)],
                    )
                }
            } else if v.class == 6 {
                (plus("AMMO_DAMAGE_TEMPLATE"), vec![Arg::F(avg)])
            } else if lo == hi {
                (plus("SINGLE_DAMAGE_TEMPLATE"), vec![Arg::D(lo.into())])
            } else {
                (
                    plus("DAMAGE_TEMPLATE"),
                    vec![Arg::D(lo.into()), Arg::D(hi.into())],
                )
            };
            // Speed rides a weapon's first emitted line only: the literal `"%s %.2f"` over
            // `Delay × 0.001`, with only the word a key.
            let speed = (first && v.class == 2).then(|| {
                let word = get("SPEED").unwrap_or_default();
                let secs = f64::from(v.delay_ms) * f64::from(0.001_f32);
                format!("{word} {secs:.2}")
            });
            // White in both cells, even for an unusable item.
            if let Some(t) = get(&key) {
                let line = (fill(&t, &args), WHITE);
                match speed {
                    Some(s) => add2(line, (s, WHITE))?,
                    None => add(line)?,
                }
            }
            // DPS sums the raw floats of the emitted slots, not the printed bounds.
            dps_acc += (max + min) * 0.5;
            first = false;
        }
        // DPS needs an emitted line and `ItemClass == 2`, so ammunition has none.
        if !first && v.class == 2 {
            // The precision is `DPS_TEMPLATE`'s own; the reference has no divide-by-zero guard
            // either.
            let secs = f64::from(v.delay_ms) * f64::from(0.001_f32);
            keyed(
                "DPS_TEMPLATE",
                &[Arg::F(f64::from(dps_acc) / secs)],
                WHITE,
                false,
            )?;
        }
        if v.armor > 0 {
            keyed("ARMOR_TEMPLATE", &[Arg::D(v.armor.into())], WHITE, false)?;
        }
        if v.block > 0 {
            keyed(
                "SHIELD_BLOCK_TEMPLATE",
                &[Arg::D(v.block.into())],
                WHITE,
                false,
            )?;
        }
        // Stats in the display order of (4,3,7,5,6,1,0, then 8,9,2,10 and zero padding), not wire
        // order. Deviation: the reference's padding repeats a mana stat once per zero entry; not
        // emulated, since only the internal test ring 6674 has one.
        const STAT_DISPLAY_ORDER: [u32; 7] = [4, 3, 7, 5, 6, 1, 0];
        for &want in &STAT_DISPLAY_ORDER {
            for &(t, val) in &v.stats {
                if t != want || val == 0 {
                    continue;
                }
                // The sign fills the template's `%c` (`"%c%d Agility"`).
                let Some(key) = stat_key(t) else { continue };
                let sign = if val > 0 { "+" } else { "-" };
                keyed(key, &[Arg::S(sign), Arg::D(val.abs().into())], WHITE, false)?;
            }
        }
        // Six equal non-zero resistances print the ALL line; otherwise one line per non-zero
        // school, Holy excluded. Field `i` is school `i + 1`.
        let first_res = v.resistances[0];
        if first_res != 0 && v.resistances.iter().all(|&r| r == first_res) {
            let sign = if first_res > 0 { "+" } else { "-" };
            keyed(
                "ITEM_RESIST_ALL",
                &[Arg::S(sign), Arg::D(first_res.abs().into())],
                WHITE,
                false,
            )?;
        } else {
            // The builder's order, not field order: the builder walks 1..5 reading school 6 for 1,
            // so Arcane comes first and Holy is displaced rather than tested.
            const RESIST_EMIT_ORDER: [u32; 5] = [6, 2, 3, 4, 5];
            for school in RESIST_EMIT_ORDER {
                let r = v.resistances[school as usize - 1];
                if r == 0 {
                    continue;
                }
                let sign = if r > 0 { "+" } else { "-" };
                let school = school_key(school).and_then(|k| get(&k)).unwrap_or_default();
                keyed(
                    "ITEM_RESIST_SINGLE",
                    &[Arg::S(sign), Arg::D(r.abs().into()), Arg::S(&school)],
                    WHITE,
                    false,
                )?;
            }
        }
        // The socket walk (`0x6277f0`): an item object or a link block walks its twelve enchant
        // slots, a bare template prints its sockets, then the placeholder for a random roll; a
        // wrapped gift shows neither.
        match inst {
            Some(i) if i.flags & 0x8 == 0 => socket_lines(lua, this, v, i, &get)?,
            Some(_) => {}
            None => {
                for &color in &v.socket_colors {
                    if let Some(name) = socket_color_name(color) {
                        empty_socket(lua, this, Some(name), GRAY, &get)?;
                    }
                }
                if let Some(bonus) = &v.socket_bonus_name {
                    keyed("ITEM_SOCKET_BONUS", &[Arg::S(bonus)], GRAY, false)?;
                }
                if v.random_property != 0 {
                    keyed("ITEM_RANDOM_ENCHANT", &[], GREEN, false)?;
                }
            }
        }
        // The druid's feral attack power off a main-hand weapon's DPS above 54.81 (`0x61a8f0`).
        // A gem's own line (`0x6277f0`): its socket enchant's name, white, red while its required
        // skill is short, gray when it is to be destroyed; then a meta gem's condition lines
        // (`0x62a994`).
        if v.gem_properties != 0 && !preview {
            if let Some(name) = v.gem.as_ref().and_then(|g| g.enchant_name.clone()) {
                let skill_ok = v.required_skill == 0
                    || req
                        .skills
                        .get(&v.required_skill)
                        .is_some_and(|&have| have >= v.required_skill_rank);
                let color = if destroy_gem {
                    GRAY
                } else {
                    req_color(skill_ok)
                };
                add((name, color))?;
            }
            if let Some(cond) = v.gem.as_ref().and_then(|g| g.condition.as_ref()) {
                let counts =
                    crate::script::socket::equipped_gem_counts(&crate::script::model_mut_of(lua));
                let (text, _) = super::sockets::condition_text(cond, counts, false, &get);
                if !text.is_empty() {
                    addw((text, WHITE))?;
                }
            }
        }
        if req.class_id == 11
            && v.class == 2
            && matches!(v.inventory_type, 13 | 17 | 21 | 22)
            && v.delay_ms != 0
        {
            let dps = f64::from(dps_acc) / (f64::from(v.delay_ms) * f64::from(0.001_f32));
            let cut = f64::from(54.81_f32);
            if dps > cut {
                feral_ap = Some(((dps - cut) * 14.0 + 0.5).floor() as i64);
            }
        }
    }
    if preview {
        // The socket preview prints no durability or duration (`0x6277f0`).
    } else if let Some((cur, max)) = inst.and_then(|i| i.durability).filter(|&(_, max)| max > 0) {
        // Red only when broken.
        let color = if cur == 0 { RED } else { WHITE };
        keyed(
            "DURABILITY_TEMPLATE",
            &[Arg::D(cur.into()), Arg::D(max.into())],
            color,
            false,
        )?;
    } else if v.max_durability > 0 {
        let full = i64::from(v.max_durability);
        keyed(
            "DURABILITY_TEMPLATE",
            &[Arg::D(full), Arg::D(full)],
            WHITE,
            false,
        )?;
    }
    // ITEM_DURATION, the item's own expiry: after durability, before ITEM_COOLDOWN_TIME, through
    // the ladder. Its prefix ships no `_P1` twin, so every count reads "days" or "hrs"
    // (`GlobalStrings.lua:2401-2404`). The reference's formatter takes a dword, so a timer past
    // 49.7 days saturates here; no 1.12 item lasts past a fortnight.
    if let Some(ms) = inst.and_then(|i| i.duration_ms).filter(|_| !preview) {
        let ms = u32::try_from(ms).unwrap_or(u32::MAX);
        if let Some(line) = duration_text(ms, "ITEM_DURATION", true, &get) {
            add((line, WHITE))?;
        }
    }
    if !preview
        && v.allowable_class > 0
        && (v.allowable_class & full_mask(&CLASS_NAMES)) != full_mask(&CLASS_NAMES)
    {
        let list: Vec<&str> = CLASS_NAMES
            .iter()
            .filter(|&&(id, _)| v.allowable_class & (1 << (id - 1)) != 0)
            .map(|&(_, n)| n)
            .collect();
        if !list.is_empty() {
            let ok = req.class_id > 0 && v.allowable_class & (1 << (req.class_id - 1)) != 0;
            keyed(
                "ITEM_CLASSES_ALLOWED",
                &[Arg::S(&list.join(", "))],
                req_color(ok),
                false,
            )?;
        }
    }
    if !preview
        && v.allowable_race > 0
        && (v.allowable_race & full_mask(&RACE_NAMES)) != full_mask(&RACE_NAMES)
    {
        let list: Vec<&str> = RACE_NAMES
            .iter()
            .filter(|&&(id, _)| v.allowable_race & (1 << (id - 1)) != 0)
            .map(|&(_, n)| n)
            .collect();
        if !list.is_empty() {
            let ok = req.race_id > 0 && v.allowable_race & (1 << (req.race_id - 1)) != 0;
            keyed(
                "ITEM_RACES_ALLOWED",
                &[Arg::S(&list.join(", "))],
                req_color(ok),
                false,
            )?;
        }
    }
    // Only a required level above 1 prints.
    if v.required_level > 1 {
        keyed(
            "ITEM_MIN_LEVEL",
            &[Arg::D(v.required_level.into())],
            req_color(req.level >= v.required_level),
            false,
        )?;
    }
    // `showItemLevel` (`0x62b1ad`): ItemClass 2, 4, 5 and 6 (weapon, armor, reagent, projectile,
    // `0x62b1cf`) take `ITEM_LEVEL` under the level line, which it follows whether or not printed.
    if show_item_level && matches!(v.class, 2 | 4 | 5 | 6) {
        keyed("ITEM_LEVEL", &[Arg::D(v.item_level.into())], WHITE, false)?;
    }
    // ITEM_MIN_SKILL with a rank, ITEM_REQ_SKILL without; the required spell takes ITEM_REQ_SKILL
    // too.
    // The requirement lines (`0x6277f0`), which the socket preview cuts; the level lines above stay.
    if let Some(skill) = v.required_skill_name.as_ref().filter(|_| !preview) {
        let have = req.skills.get(&v.required_skill).copied().unwrap_or(0);
        let color = req_color(have >= v.required_skill_rank.max(1));
        if v.required_skill_rank > 0 {
            let args = [Arg::S(skill), Arg::D(v.required_skill_rank.into())];
            keyed("ITEM_MIN_SKILL", &args, color, false)?;
        } else {
            keyed("ITEM_REQ_SKILL", &[Arg::S(skill)], color, false)?;
        }
    }
    if v.required_spell != 0 && !preview {
        if let Some(name) = &v.required_spell_name {
            keyed(
                "ITEM_REQ_SKILL",
                &[Arg::S(name)],
                req_color(known_spell),
                false,
            )?;
        }
    }
    if let Some(rep) = v.required_rep_line.as_ref().filter(|_| !preview) {
        // Red below the required rank (`[+0x58]`); an unfed faction reads as unmet.
        let rank = req
            .rep_ranks
            .get(&v.required_rep_faction)
            .copied()
            .unwrap_or(0);
        add((
            rep.clone(),
            req_color(u32::from(rank) >= v.required_rep_rank),
        ))?;
    }
    if taught_known && !preview {
        keyed("ITEM_SPELL_KNOWN", &[], RED, false)?;
    }
    // The green stat lines (`0x6277f0`): the feral line, then each rating stat in wire order as the
    // ON_EQUIP prefix and its `ITEM_MOD_*` text, joined by the literal `"%s %s"`.
    if let (Some(ap), Some(t)) = (feral_ap, get("FERAL_DRUID_ITEM_AP")) {
        addw((fill(&t, &[Arg::D(ap)]), GREEN))?;
    }
    if let Some(prefix) = get("ITEM_SPELL_TRIGGER_ONEQUIP") {
        for &(t, val) in &v.stats {
            let Some(template) = rating_key(t).and_then(&get) else {
                continue;
            };
            if val == 0 {
                continue;
            }
            // A `%c` template takes the sign; the rest take the value alone.
            let text = if template.contains("%c") {
                let sign = if val > 0 { "+" } else { "-" };
                fill(&template, &[Arg::S(sign), Arg::D(val.abs().into())])
            } else {
                fill(&template, &[Arg::D(val.into())])
            };
            add((format!("{prefix} {text}"), GREEN))?;
        }
    }
    // Green, wrapped trigger lines: the prefix key and the app-resolved text joined by the literal
    // `"%s %s"`.
    for &(trigger, _, ref text) in &v.spell_triggers {
        let key = match trigger {
            0 | 5 => "ITEM_SPELL_TRIGGER_ONUSE",
            1 => "ITEM_SPELL_TRIGGER_ONEQUIP",
            2 => "ITEM_SPELL_TRIGGER_ONPROC",
            _ => continue,
        };
        let Some(prefix) = get(key) else { continue };
        addw((format!("{prefix} {text}"), GREEN))?;
    }
    if let Some(charges) = charges_phrase(v.charges.max(0) as u32, &get) {
        add((charges, WHITE))?;
    }
    // The set block, above p4's cut: a blank gold line, the gold "name (worn/total)" header, the
    // set's skill line, the members (a worn one cream under its own name, else gray; one still in
    // flight waits), a second blank, then the bonuses sorted by threshold, green when the skill is
    // met and enough are worn. A met bonus is ITEM_SET_BONUS, the text alone; only an unmet one is
    // ITEM_SET_BONUS_GRAY, with its count.
    if let Some(set) = set_view.as_ref().filter(|_| !preview) {
        let count = count_set(v.item_set, &set.members, &worn);
        addw((SET_SPACER.into(), GOLD))?;
        keyed(
            "ITEM_SET_NAME",
            &[
                Arg::S(&set.name),
                Arg::D(count.worn as i64),
                Arg::D(count.total as i64),
            ],
            GOLD,
            false,
        )?;
        let skill_met = set.required_skill == 0 || {
            let have = req.skills.get(&set.required_skill).copied().unwrap_or(0);
            have >= set.required_skill_rank
        };
        if let Some(skill) = &set.required_skill_name {
            let color = req_color(skill_met);
            if set.required_skill_rank > 0 {
                let args = [Arg::S(skill), Arg::D(set.required_skill_rank.into())];
                keyed("ITEM_MIN_SKILL", &args, color, false)?;
            } else {
                keyed("ITEM_REQ_SKILL", &[Arg::S(skill)], color, false)?;
            }
        }
        // The two-space indent is the literal `"  %s"`, not a key.
        for (member, piece) in set.members.iter().zip(&count.members) {
            match piece {
                Some(name) => add((format!("  {name}"), CREAM))?,
                None => {
                    let Some(name) = &member.name else { continue };
                    add((format!("  {name}"), GRAY))?;
                }
            }
        }
        addw((SET_SPACER.into(), GOLD))?;
        let mut bonuses: Vec<&(u32, String)> = set.bonuses.iter().collect();
        bonuses.sort_by_key(|&&(threshold, _)| threshold);
        for &(threshold, ref text) in bonuses {
            if skill_met && count.worn as u32 >= threshold {
                keyed("ITEM_SET_BONUS", &[Arg::S(text)], GREEN, true)?;
            } else {
                let args = [Arg::D(threshold.into()), Arg::S(text)];
                keyed("ITEM_SET_BONUS_GRAY", &args, GRAY, true)?;
            }
        }
    }
    // p4's early return: compact falls through, lays out and returns, so nothing from the rest of
    // the builder runs: flavor text, creator, open or read line, money.
    if flags.name_only {
        return Ok(());
    }
    if !v.description.is_empty() {
        addw((format!("\"{}\"", v.description), GOLD))?;
    }
    // The reference gates the instance lines on an item object. Here only a template source
    // (`None`) skips them, so a link, loot, mail or auction hover, which has an instance but no
    // object, still reaches READABLE.
    if let Some(inst) = inst {
        // The creator line, white: ITEM_WRITTEN_BY for a letter, else ITEM_CREATED_BY, green by its
        // own `|cff00ff00`. A wrapped instance (`0x8`) takes ITEM_WRAPPED_BY off
        // `ITEM_FIELD_GIFTCREATOR` in the reference; that is not built, so a wrapped instance
        // prints no creator line.
        if inst.flags & 0x8 == 0 && inst.socket_preview.is_none() {
            if let Some(name) = &inst.creator {
                let key = if inst.has_text {
                    "ITEM_WRITTEN_BY"
                } else {
                    "ITEM_CREATED_BY"
                };
                keyed(key, &[Arg::S(name)], WHITE, false)?;
            }
        }
        // One green line, OPENABLE winning. Openable: a p6=0 source and either the loot flag `0x4`
        // behind its lock gate (a lock needs UNLOCKED) or a wrapped gift (`0x200` and WRAPPED
        // `0x8`). Readable: PageText or the letter text. The lock gate is the line's only: the
        // click tests the bare flag (`ItemInfo::opens_loot`), and READABLE wins there.
        let openable = inst.openable_source
            && ((v.flags & 0x4 != 0 && (v.lock_id == 0 || inst.flags & 0x4 != 0))
                || (v.flags & 0x200 != 0 && inst.flags & 0x8 != 0));
        if openable {
            keyed("ITEM_OPENABLE", &[], GREEN, false)?;
        } else if v.page_text != 0 || inst.has_text {
            keyed("ITEM_READABLE", &[], GREEN, false)?;
        }
        // The equipment-set line (`0x62d35a`-`0x62d3fc`): with `equipmentManager` on and
        // `dontShowEquipmentSetsOnItems` off, the sets holding this item, wrapped.
        if inst.guid != 0 {
            let list = {
                let model = crate::script::model_mut_of(lua);
                let on = |n: &str| {
                    model
                        .cvars
                        .get(n)
                        .is_some_and(|c| crate::script::cvars::sstr_to_int(&c.value) != 0)
                };
                (on("equipmentmanager") && !on("dontshowequipmentsetsonitems"))
                    .then(|| {
                        equipment_set_list(
                            &model.equipment_manager.sets,
                            inst.guid,
                            &get("PLAYER_LIST_DELIMITER").unwrap_or_default(),
                        )
                    })
                    .flatten()
            };
            if let Some(list) = list {
                keyed("EQUIPMENT_SETS", &[Arg::S(&list)], GOLD, true)?;
            }
        }
    }
    // The trade lines (`0x6277f0`): an open refund window's countdown, which takes the money
    // block's place, then a bound item's trade window.
    let mut refundable = false;
    if let Some(inst) = inst.filter(|i| i.guid != 0) {
        let (refund_left, trade_left) = {
            let model = crate::script::model_mut_of(lua);
            let enchanted = inst
                .enchants
                .iter()
                .any(|e| matches!(e.slot, 0 | 2 | 3 | 6));
            let refund =
                crate::script::refund::seconds_left(&model, inst.guid).filter(|_| !enchanted);
            let trade = inst
                .already_bound
                .then(|| crate::script::refund::bound_trade_left(&model, inst.guid))
                .flatten();
            (refund, trade)
        };
        for (left, key) in [
            (refund_left, "REFUND_TIME_REMAINING"),
            (trade_left, "BIND_TRADE_TIME_REMAINING"),
        ] {
            let Some(left) = left else { continue };
            if let (Some(time), Some(t)) = (window_text(left, &get), get(key)) {
                add((" ".into(), GOLD))?;
                addw((fill(&t, &[Arg::S(&time)]), REFUND_BLUE))?;
            }
        }
        refundable = refund_left.is_some();
    }
    if refundable {
        return Ok(());
    }
    add_sell_price(lua, this, v, flags.sell)
}

/// A trade or refund window's time left (`0x6277f0`): `INT_SPELL_DURATION`'s largest unit, except
/// that 61 minutes to under two hours reads "1 hour" and the minutes past it.
fn window_text(left: i64, get: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    let left = u32::try_from(left).ok()?;
    if !(3660..7200).contains(&left) {
        return duration_text(left.saturating_mul(1000), "INT_SPELL_DURATION", false, get);
    }
    let hours = plural_template("INT_SPELL_DURATION_HOURS", 1, get)?.replacen("%d", "1", 1);
    let n = (left - 3600) / 60;
    let minutes =
        plural_template("INT_SPELL_DURATION_MIN", n, get)?.replacen("%d", &n.to_string(), 1);
    let delimiter = get("TIME_UNIT_DELIMITER").unwrap_or_default();
    Some(format!("{hours}{delimiter}{minutes}"))
}

/// The money block, the builder's last lines (`0x62d71b..0x62d8ef`): nothing in repair mode
/// (cursor mode 17) or for a `skipSellPrice` source, else `OnTooltipAddMoney(cost, maxcost)` with
/// no merchant check. At price 0 an item object outside the worn slots prints ITEM_UNSELLABLE,
/// only while a vendor is open (`0x62d823`). The charge scaling `0x62d772` is not built.
fn add_sell_price(lua: &Lua, this: &Table, v: &ItemTemplateView, sell: Sell) -> mlua::Result<()> {
    if sell == Sell::Skip {
        return Ok(());
    }
    let (repairing, merchant_open) = {
        let model = crate::script::model_mut_of(lua);
        (model.repair_mode, model.merchant.is_some())
    };
    if repairing {
        return Ok(());
    }
    let price = u64::from(v.sell_price);
    if price == 0 {
        if merchant_open && matches!(sell, Sell::Object { worn: false, .. }) {
            if let Some(t) = crate::strings::global(lua, "ITEM_UNSELLABLE") {
                append_line(lua, this, (t, WHITE), None, false)?;
            }
        }
        return Ok(());
    }
    let (cost, max) = match sell {
        Sell::Skip => return Ok(()),
        Sell::Object {
            stack, repair_cost, ..
        } => {
            let repair = u64::from(repair_cost);
            let unit = if repair < price { price - repair } else { 1 };
            (unit * u64::from(stack.max(1)), None)
        }
        Sell::Block { stack, max } => (
            price * u64::from(stack),
            max.filter(|&m| m > stack).map(|m| price * u64::from(m)),
        ),
    };
    let h = crate::script::object::frame_handle_of(lua, this)?;
    super::fire_add_money(lua, h, cost, max);
    Ok(())
}

/// A temporary enchant's name with its countdown: `ITEM_ENCHANT_TIME_LEFT_<unit>`, its `_P1` twin
/// picked by [`plural_template`]. Two holes, the name then the count, so not `duration_text`;
/// `None` when the install lacks the family, and the caller draws no line.
pub(super) fn enchant_time_left(
    name: &str,
    ms: u64,
    get: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    let (suffix, n) = time_bucket(ms);
    let template = plural_template(&format!("ITEM_ENCHANT_TIME_LEFT_{suffix}"), n, get)?;
    Some(fill(&template, &[Arg::S(name), Arg::D(i64::from(n))]))
}

/// The reference's time-text unit and count: the largest unit that fits, the count ceiled for days,
/// hours and minutes (all seven callers pass `roundUp` 1) and truncated for seconds.
fn time_bucket(ms: u64) -> (&'static str, u32) {
    const SEC: u64 = 1_000;
    const MIN: u64 = 60 * SEC;
    const HOUR: u64 = 60 * MIN;
    const DAY: u64 = 24 * HOUR;
    let (suffix, n) = match ms {
        _ if ms >= DAY => ("DAYS", ms.div_ceil(DAY)),
        _ if ms >= HOUR => ("HOURS", ms.div_ceil(HOUR)),
        _ if ms >= MIN => ("MIN", ms.div_ceil(MIN)),
        _ => ("SEC", ms / SEC),
    };
    // The count is a dword in the reference; a timer long enough to overflow one does not exist.
    (suffix, u32::try_from(n).unwrap_or(u32::MAX))
}

/// `ITEM_SPELL_CHARGES` or its `_P1` twin, pushed for the enchant suffix and for the charges line;
/// zero charges have no phrase.
pub(super) fn charges_phrase(n: u32, get: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    if n == 0 {
        return None;
    }
    let template = plural_template("ITEM_SPELL_CHARGES", n, get)?;
    Some(fill(&template, &[Arg::D(i64::from(n))]))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stand-in string table, not the shipped wording: the tests check which key is reached and
    /// what fills it.
    fn table(key: &str) -> Option<String> {
        Some(
            match key {
                "ITEM_ENCHANT_TIME_LEFT_DAYS" => "<%s :: %d D>",
                "ITEM_ENCHANT_TIME_LEFT_DAYS_P1" => "<%s :: %d DD>",
                "ITEM_ENCHANT_TIME_LEFT_HOURS" => "<%s :: %d H>",
                "ITEM_ENCHANT_TIME_LEFT_HOURS_P1" => "<%s :: %d HH>",
                "ITEM_ENCHANT_TIME_LEFT_MIN" => "<%s :: %d M>",
                "ITEM_ENCHANT_TIME_LEFT_SEC" => "<%s :: %d S>",
                "ITEM_SPELL_CHARGES" => "<%d chg>",
                "ITEM_SPELL_CHARGES_P1" => "<%d chgs>",
                _ => return None,
            }
            .to_string(),
        )
    }

    /// The ladder: minutes and seconds have no `_P1` twin, so each reaches one key.
    #[test]
    fn enchant_countdown_buckets_and_rounding() {
        let t = |ms| enchant_time_left("Rockbiter", ms, &table);
        // Seconds truncate: 1900 ms is 1, not 2.
        assert_eq!(t(1_900).as_deref(), Some("<Rockbiter :: 1 S>"));
        assert_eq!(t(59_999).as_deref(), Some("<Rockbiter :: 59 S>"));
        // Minutes ceil: one second past 4 minutes already reads 5.
        assert_eq!(t(60_000).as_deref(), Some("<Rockbiter :: 1 M>"));
        assert_eq!(t(241_000).as_deref(), Some("<Rockbiter :: 5 M>"));
        // Hours ceil, and only the non-singular count takes the `_P1` key.
        assert_eq!(t(3_600_000).as_deref(), Some("<Rockbiter :: 1 H>"));
        assert_eq!(t(3_600_001).as_deref(), Some("<Rockbiter :: 2 HH>"));
        // Days ceil.
        assert_eq!(t(86_400_000).as_deref(), Some("<Rockbiter :: 1 D>"));
        assert_eq!(t(86_400_001).as_deref(), Some("<Rockbiter :: 2 DD>"));
        // The app drops an expired timer, but zero must not panic; it takes the plural arm, the
        // reference's plural-index rule.
        assert_eq!(t(0).as_deref(), Some("<Rockbiter :: 0 S>"));
    }

    #[test]
    fn a_missing_countdown_family_draws_nothing() {
        assert_eq!(enchant_time_left("Rockbiter", 60_000, &|_| None), None);
        assert_eq!(charges_phrase(3, &|_| None), None);
    }

    #[test]
    fn charges_phrase_picks_the_plural() {
        assert_eq!(charges_phrase(1, &table).as_deref(), Some("<1 chg>"));
        assert_eq!(charges_phrase(5, &table).as_deref(), Some("<5 chgs>"));
        assert_eq!(charges_phrase(0, &table), None);
    }

    /// `0x5ae380`: each set holding the item, in set order, once per slot that holds it.
    #[test]
    fn the_set_list_names_each_set_holding_the_item() {
        let set = |name: &str, items: &[(usize, u64)]| {
            let mut v = crate::script::EquipmentSetView {
                name: name.into(),
                ..Default::default()
            };
            for &(slot, guid) in items {
                v.items[slot] = guid;
            }
            v
        };
        let sets = [
            set("Tank", &[(0, 7), (15, 9)]),
            set("Heal", &[(15, 8)]),
            set("Rings", &[(10, 7), (11, 7)]),
        ];
        assert_eq!(
            equipment_set_list(&sets, 7, ", ").as_deref(),
            Some("Tank, Rings, Rings")
        );
        assert_eq!(equipment_set_list(&sets, 8, ", ").as_deref(), Some("Heal"));
        assert_eq!(equipment_set_list(&sets, 5, ", "), None);
    }
}
