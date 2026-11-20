//! The spell tooltip view: the reference's spell tooltip builder's cells, resolved where the
//! catalogs live and built at the hover from the state as it is then, as the reference builds it
//! whole at the call and keeps nothing.

use benilla_ui::strings::Arg;

use super::keyed;
use crate::items::Items;
use crate::net::{NetCommands, ObjectStore, Objects};
use crate::spell::usable;
use crate::ui_action::Spells;
use crate::ui_items::{count_of, InventoryScope};

/// What a view reads beyond the live view's own state: the bind point `$z` names and the
/// required-item line's subclass names.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct TooltipTables<'w> {
    home_bind: Option<bevy::prelude::Res<'w, crate::net::HomeBind>>,
    area_names: Option<bevy::prelude::Res<'w, crate::ui_quest_log::QuestHeaderNamesRes>>,
    sub_classes: Option<bevy::prelude::Res<'w, crate::ui_items::ItemSubClasses>>,
}

impl TooltipTables<'_> {
    /// The bind point's area name (Astral Recall's `$z`).
    pub(crate) fn home_area(&self) -> Option<&str> {
        let id = self.home_bind.as_deref()?.0?;
        self.area_names.as_deref()?.0.resolve(id as i32)
    }

    pub(crate) fn sub_classes(&self) -> Option<&benilla_formats::ItemSubClassCatalog> {
        self.sub_classes.as_deref().map(|c| &c.0)
    }
}

/// The unit a view's caster cells are computed against, the spell tooltip builder's fourth
/// argument: the level the `$`-tokens, the cost and the cast time scale by, and the cost's basis.
#[derive(Clone, Copy)]
pub(crate) enum ViewCaster<'a> {
    /// The active player ([`ViewCtx::store`]), selector 0.
    Player,
    /// Selector 1: the player's charm, else its summon, `None` when neither is streamed.
    Pet(Option<&'a benilla_protocol::ObjectFields>),
}

impl ViewCaster<'_> {
    /// The pet's `[vtbl+0xa8]`, the unit's skill-value slot: `UNIT_FIELD_LEVEL × 5`, which
    /// [`benilla_formats::SpellDisplay::skill_level`] caps at `maxLevel × 5` and divides by 5 as
    /// the reference does. No unit reads 0.
    fn pet_skill_value(pet: Option<&benilla_protocol::ObjectFields>) -> u32 {
        pet.and_then(|p| p.unit_level())
            .unwrap_or(0)
            .saturating_mul(5)
    }
}

/// The caster-dependent inputs of a spell view: the worn set, the bags, the form and the bind point
/// `$z` names.
pub(crate) struct ViewCtx<'a, 'w, 's> {
    pub(crate) home_area: Option<&'a str>,
    /// The caster's shapeshift form, which the required-form line's colour reads.
    pub(crate) form: u8,
    /// The active player.
    pub(crate) store: Option<&'a ObjectStore>,
    /// Whom the level terms and the cost read: the player or its pet. The form and the reaches
    /// are that same unit's.
    pub(crate) caster: ViewCaster<'a>,
    /// The caster as the range getter reads it: its `UNIT_FIELD_COMBATREACH` (1.5 is the descriptor
    /// default) and its motion.
    pub(crate) range_caster: benilla_formats::RangeUnit,
    /// The caster's auto-attack target, which the range getter looks up itself (`[caster+0xc48]`)
    /// for its melee arm alone, reading its reach and motion; with none, the caster's reach counts
    /// twice and there is no moving bonus.
    pub(crate) attack_target: Option<benilla_formats::RangeUnit>,
    /// The object index the worn-item search and each reagent's carried count resolve through.
    pub(crate) objects: &'a Objects<'w, 's>,
    pub(crate) items: &'a Items,
    pub(crate) commands: &'a NetCommands,
    pub(crate) sub_classes: Option<&'a benilla_formats::ItemSubClassCatalog>,
    /// The spell-to-line hop the `$`-tokens' skill level reads ([`crate::spell::spell_skill_value`]).
    pub(crate) skill_lines: Option<&'a benilla_formats::SkillLineCatalog>,
    /// The talent spell-modifier tables: the cost cell shows the modified cost (`power_cost`).
    pub(crate) spell_mods: &'a crate::spell::SpellModifiers,
    /// The VM's `GlobalStrings.lua`, where the keyed cells and the `$` tokens' templates resolve.
    pub(crate) get: &'a dyn Fn(&str) -> Option<String>,
    /// The player's known spells, which a description's `$?s` reads.
    pub(crate) known: &'a std::collections::BTreeSet<u32>,
    /// The player's auras, which a description's `$?a` reads.
    pub(crate) auras: Option<&'a crate::unit_auras::UnitAuras>,
    /// The expression operands off the player's fields, this frame's.
    pub(crate) operands: &'a crate::spell::text_caster::Operands,
}

/// Build one spell's tooltip view, every cell resolved here where the catalogs live, and what it
/// The range, cast, cooldown, form and item cells fill their `GlobalStrings.lua`
/// keys; the cost, reagents and chance cells are composed in English, where the reference fills
/// `MANA_COST` and its kin, `SPELL_REAGENTS` and `CHANCE_TO_*`.
pub(crate) fn build_view(
    spell_id: u32,
    spells: &Spells,
    vctx: &mut ViewCtx,
) -> Option<benilla_ui::script::SpellTooltipView> {
    let d = spells.catalog.get(spell_id)?;
    let form = vctx.form;
    let caster = vctx.caster;
    // The `$`-tokens' level, by the view's caster selector.
    let skill = |id| match caster {
        ViewCaster::Player => {
            let (race, class) = crate::spell::race_class(vctx.store);
            let line = vctx
                .skill_lines
                .and_then(|c| c.spell_to_line(id, race, class));
            line.map_or(0, |line| crate::spell::skill_line_value(vctx.store, line))
        }
        ViewCaster::Pet(pet) => ViewCaster::pet_skill_value(pet),
    };
    let lookup = |id| spells.catalog.get(spells.for_difficulty(id));
    let home = vctx.home_area;
    let home_area = || home;
    // `$g`/`$G` read the active player's gender byte, a pet view's too.
    let gender = || vctx.store.and_then(|s| s.0.unit_gender()).unwrap_or(0);
    // The caster's skill level for this spell: the level the cost's and the cast time's per-level
    // terms read.
    let pet_level = |pet| d.skill_level(ViewCaster::pet_skill_value(pet));
    // `$c` reads the view's caster's cost, as the cost cell does.
    let text_cost = |t: &benilla_formats::SpellDisplay| match caster {
        ViewCaster::Player => vctx
            .store
            .map_or(t.mana_cost, |s| usable::power_cost(t, s, vctx.spell_mods))
            as i32,
        ViewCaster::Pet(pet) => pet.map_or(-1, |p| {
            usable::power_cost_at(
                t,
                p,
                t.skill_level(ViewCaster::pet_skill_value(Some(p))),
                vctx.spell_mods,
            ) as i32
        }),
    };
    let text_caster = vctx
        .store
        .map(|store| crate::spell::text_caster::TextCaster {
            store,
            known: vctx.known,
            auras: vctx.auras,
            operands: vctx.operands,
            mods: vctx.spell_mods,
            skill: &skill,
            power_cost: &text_cost,
        });
    let ctx = benilla_formats::TokenContext {
        durations: &spells.durations,
        radii: &spells.radii,
        ranges: Some(&spells.ranges),
        variables: Some(spells.catalog.description_variables()),
        skill: &skill,
        lookup: &lookup,
        mods: Some(vctx.spell_mods),
        unmodified_points: false,
        gender: &gender,
        home_area: &home_area,
        caster: text_caster
            .as_ref()
            .map(|c| c as &dyn benilla_formats::Caster),
        global: vctx.get,
        printf: &crate::ui_script::token_printf,
    };
    // The aura tooltip expands the aura text with the points' modifiers off.
    let aura_ctx = benilla_formats::TokenContext {
        unmodified_points: true,
        ..ctx
    };
    // The cost cell: the resolved `power_cost`, named by power type and Health for a type outside
    // 0..=4, never a percentage. Rage shows wire ÷ 10, a per-second column adds `_PER_TIME`, and no
    // cost at all leaves the cell empty. A pet with no unit costs -1, which shows as none.
    let resolved_cost = match caster {
        ViewCaster::Player => vctx
            .store
            .map_or(d.mana_cost, |s| usable::power_cost(d, s, vctx.spell_mods)),
        ViewCaster::Pet(pet) => pet.map_or(0, |p| {
            usable::power_cost_at(d, p, pet_level(Some(p)), vctx.spell_mods)
        }),
    };
    // The spell's `PowerDisplay.dbc` row and the power's name from its tag (`0x6238a0`).
    let display_noun = spells
        .power_displays
        .get(d.power_display_id)
        .map(|row| (vctx.get)(&row.tag).unwrap_or_else(|| row.tag.clone()));
    let cost = {
        // The shared per-type divisor, never a local `power_type == 1` test.
        let div = benilla_protocol::messages::power_display_scale(d.power_type);
        let unit = match d.power_type {
            0 => "Mana",
            1 => "Rage",
            2 => "Focus",
            3 => "Energy",
            4 => "Happiness",
            _ => "Health",
        };
        if resolved_cost == 0 && d.mana_per_second == 0 {
            None
        } else if let Some(noun) = &display_noun {
            if d.mana_per_second > 0 {
                keyed(
                    vctx.get,
                    "POWER_DISPLAY_COST_PER_TIME",
                    &[
                        Arg::D(i64::from(resolved_cost / div)),
                        Arg::S(noun),
                        Arg::D(i64::from(d.mana_per_second / div)),
                    ],
                )
            } else {
                keyed(
                    vctx.get,
                    "POWER_DISPLAY_COST",
                    &[Arg::D(i64::from(resolved_cost / div)), Arg::S(noun)],
                )
            }
        } else if d.mana_per_second > 0 {
            Some(format!(
                "{} {unit}, plus {} per sec",
                resolved_cost / div,
                d.mana_per_second / div
            ))
        } else {
            Some(format!("{} {unit}", resolved_cost / div))
        }
    };
    // The range cell: two attribute gates, the range getter, then `max <= 0`. There is no melee
    // wording: the melee row (flags bit 0, row 2 only) resolves to
    // `max(reach + casterReach + 1.3333334, 5.0)` and prints like any other, usually "5 yd range".
    let range = (!d.tooltip_omits_range_line())
        .then(|| {
            let row = spells.ranges.get(d.range_index);
            // The call passes a null target: the auto-attack target is the melee arm's own lookup,
            // and the ranged arm, with no target, pads nothing.
            let targets = benilla_formats::RangeTargets {
                target: None,
                attack_target: vctx.attack_target,
            };
            let (min, max) = vctx
                .spell_mods
                .min_max_range(d, row, vctx.range_caster, targets)?;
            if max <= 0.0 {
                return None;
            }
            // `SPELL_RANGE`'s hole is a string: the number is composed first, `"%d"` or `"%d-%d"`
            // (Charge's 8-25), each a `fistp` conversion that rounds to nearest.
            let yd = |v: f32| f64::from(v).round_ties_even() as i64;
            let yards = if min > 0.0 {
                format!("{}-{}", yd(min), yd(max))
            } else {
                format!("{}", yd(max))
            };
            keyed(vctx.get, "SPELL_RANGE", &[Arg::S(&yards)])
        })
        .flatten();
    // The cast line: its gate also omits it for a TRADE_SKILL or ATTACK `Effect[0]`; then the
    // ladder over the cast-time getter with selector 1, unclamped, where a negative time is
    // "Instant cast" and a zero one is "Instant cast" only for a mana spell with a cost.
    let cast_time = if d.tooltip_omits_cast_line() {
        None
    } else {
        // The cast-time getter takes the selector too, and its level term is the skill level's.
        let level = match caster {
            ViewCaster::Player => vctx.store.and_then(|s| s.0.unit_level()).unwrap_or(0),
            ViewCaster::Pet(pet) => pet_level(pet),
        };
        let base = spells.cast_time_unclamped_ms(d, level, vctx.spell_mods);
        // `%.3g` templates: the seconds go over as a real.
        if base > 0 {
            let (key, v) = if base >= 60_000 {
                ("SPELL_CAST_TIME_MIN", f64::from(base) / 60_000.0)
            } else {
                ("SPELL_CAST_TIME_SEC", f64::from(base) / 1000.0)
            };
            keyed(vctx.get, key, &[Arg::F(v)])
        } else {
            let key = if base < 0 {
                "SPELL_CAST_TIME_INSTANT"
            } else if d.on_next_swing() {
                "SPELL_ON_NEXT_SWING"
            } else if d.tooltip_on_next_ranged() {
                "SPELL_ON_NEXT_RANGED"
            } else if d.tooltip_channeled() {
                "SPELL_CAST_CHANNELED"
            } else if d.power_type == 0 && resolved_cost > 0 {
                "SPELL_CAST_TIME_INSTANT"
            } else {
                "SPELL_CAST_TIME_INSTANT_NO_MANA"
            };
            keyed(vctx.get, key, &[])
        }
    };
    // The larger recovery column: op 11 follows the ranged-speed category pad. The ranged attack
    // time is the player's, a pet view's too.
    let ranged_ms = if d.ranged_speed_cooldown() {
        vctx.store
            .and_then(|s| s.0.unit_ranged_attack_time())
            .unwrap_or(0)
    } else {
        0
    };
    let (own_recovery, category_recovery) = vctx.spell_mods.spell_cooldowns(d, ranged_ms);
    let recovery_ms = own_recovery.max(category_recovery);
    let cooldown = (recovery_ms > 0)
        .then(|| {
            let secs = f64::from(recovery_ms) / 1000.0;
            let (key, v) = if secs >= 60.0 {
                ("SPELL_RECAST_TIME_MIN", secs / 60.0)
            } else {
                ("SPELL_RECAST_TIME_SEC", secs)
            };
            keyed(vctx.get, key, &[Arg::F(v)])
        })
        .flatten();
    // The required-form line. With `AttributesEx2` bit 19 set the mask is permissive (Inner Fire in
    // Shadowform), and the line is skipped.
    let requires_form = (d.stances != 0 && !d.form_mask_is_permissive())
        .then(|| {
            let names: Vec<&str> = (0..32u32)
                .filter(|b| d.stances & (1 << b) != 0)
                .filter_map(|b| spells.forms.get(&(b + 1)).map(|f| f.name.as_str()))
                .filter(|n| !n.is_empty())
                .collect();
            (!names.is_empty())
                .then(|| {
                    keyed(
                        vctx.get,
                        "SPELL_REQUIRED_FORM",
                        &[Arg::S(&names.join(", "))],
                    )
                })
                .flatten()
        })
        .flatten();
    // No caster unit passes the form test; a spell with no stance mask fails it for every form.
    let form_met = matches!(caster, ViewCaster::Pet(None))
        || (form != 0 && d.stances & (1u32 << (u32::from(form) - 1)) != 0);
    // The required-item line: a mask is named whole by `ItemSubClassMask.dbc` before its subclasses
    // are joined; an empty mask or class < 0 skips it.
    let requires_item = (d.targets & TARGET_ITEM == 0 && d.equipped_item_class >= 0)
        .then(|| {
            vctx.sub_classes?
                .requirement_name(d.equipped_item_class as u32, d.equipped_item_subclass_mask)
        })
        .flatten()
        .and_then(|name| keyed(vctx.get, "SPELL_EQUIPPED_ITEM", &[Arg::S(&name)]));
    let chance = chance_line(d, vctx.store);
    // The unit selector passes the item test outright.
    let item_met = matches!(caster, ViewCaster::Pet(_))
        || vctx.store.is_none_or(|s| {
            usable::equipped_item_fits(d, s, vctx.objects, vctx.items, vctx.commands)
        });
    // Reagents (`SPELL_REAGENTS`), red when short; one whose template has not streamed is absent,
    // and its template is asked for.
    let mut reagents_met = true;
    let reagents = {
        let mut parts: Vec<String> = Vec::new();
        for (entry, count) in d.reagents.iter().copied().filter(|&(e, _)| e != 0) {
            let Some(name) = vctx
                .items
                .template(entry, 0, vctx.commands)
                .map(|t| t.name.clone())
            else {
                continue;
            };
            let text = if count > 1 {
                format!("{name} ({count})")
            } else {
                name
            };
            let short = vctx.store.is_some_and(|s| {
                count_of(&s.0, vctx.objects, entry, InventoryScope::CARRIED) < count
            });
            reagents_met &= !short;
            parts.push(if short {
                format!("|cffff2020{text}|r")
            } else {
                text
            });
        }
        (!parts.is_empty()).then(|| format!("Reagents: {}", parts.join(", ")))
    };
    // `AttributesEx & 2`'s line (`0x625122`): the display's name, else the power's own key.
    let use_all_power = (d.attributes_ex & 2 != 0)
        .then(|| match &display_noun {
            Some(noun) => keyed(vctx.get, "SPELL_USE_ALL_POWER_DISPLAY", &[Arg::S(noun)]),
            None => match d.power_type {
                0 => keyed(vctx.get, "SPELL_USE_ALL_MANA", &[]),
                1 => keyed(vctx.get, "SPELL_USE_ALL_RAGE", &[]),
                2 => keyed(vctx.get, "SPELL_USE_ALL_FOCUS", &[]),
                3 => keyed(vctx.get, "SPELL_USE_ALL_ENERGY", &[]),
                4 => keyed(vctx.get, "SPELL_USE_ALL_HAPPINESS", &[]),
                5 => keyed(vctx.get, "SPELL_USE_ALL_RUNE", &[]),
                6 => None,
                _ => keyed(vctx.get, "SPELL_USE_ALL_HEALTH", &[]),
            },
        })
        .flatten();
    let view = benilla_ui::script::SpellTooltipView {
        use_all_power,
        name: d.name.clone(),
        rank: d.rank.clone(),
        // The aura variant's right column, gated on `SpellDispelType.dbc` `[+0x28]`.
        dispel_type: spells.catalog.dispel_name(d).map(str::to_string),
        cost,
        range,
        cast_time,
        cooldown,
        requires_item,
        item_met,
        requires_form,
        form_met,
        chance,
        reagents,
        reagents_met,
        description: d
            .description
            .as_deref()
            .map(|t| benilla_formats::substitute(t, d, &ctx))
            .unwrap_or_default(),
        aura_description: d
            .aura_description
            .as_deref()
            .map(|t| benilla_formats::substitute(t, d, &aura_ctx))
            .unwrap_or_default(),
    };
    Some(view)
}

/// `Targets` bit `0x10` (name inferred): set, the required-item line is skipped.
const TARGET_ITEM: u32 = 0x10;

/// The four `Effect[0]` values that select a chance-to-X line.
const EFFECT_DODGE: u32 = 20;
const EFFECT_PARRY: u32 = 22;
const EFFECT_BLOCK: u32 = 23;
const EFFECT_ATTACK: u32 = 78;

/// The chance-to-X line: ATTACK skips the passive gate that dodge, parry and block need. The wire's
/// values are already percents.
fn chance_line(d: &benilla_formats::SpellDisplay, store: Option<&ObjectStore>) -> Option<String> {
    let fields = &store?.0;
    let (percent, label) = match d.effects[0] {
        EFFECT_ATTACK => (fields.player_crit_percentage(), "crit"),
        EFFECT_DODGE if d.passive => (fields.player_dodge_percentage(), "dodge"),
        EFFECT_PARRY if d.passive => (fields.player_parry_percentage(), "parry"),
        EFFECT_BLOCK if d.passive => (fields.player_block_percentage(), "block"),
        _ => return None,
    };
    // The enUS text of the reference's `CHANCE_TO_*` keys (`%.2f%% chance to crit`); the wire's
    // values are already percents.
    Some(format!("{:.2}% chance to {label}", percent?))
}
