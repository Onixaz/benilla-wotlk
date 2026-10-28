//! The trainer window's per-service laws: its icon, tooltip subject, group key and the getters'
//! answers the app resolves from `Spell.dbc` and the player.

use std::collections::BTreeSet;

use benilla_formats::{
    LearnEffect, SkillLineCatalog, SpellCatalog, SPELL_ATTR_IS_TRADESKILL,
    SPELL_EFFECT_LEARN_PET_SPELL, SPELL_EFFECT_LEARN_SPELL, SPELL_EFFECT_SKILL_STEP,
};
use benilla_protocol::messages::trainer_spell_state;
use benilla_ui::script::{
    TrainerAbilityReq, TrainerServiceCategory, TrainerSkillReq, TrainerTooltip,
};

use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::net::NetCommands;
use crate::ui_items::item_icon;

use super::reeval::{known_or_higher, PlayerView};
use super::TRAINER_TYPE_TRADESKILL;

/// `Attributes & 0x40`, which files a tradeskill service under `TRADESKILL_SERVICE_PASSIVE`.
const SPELL_ATTR_PASSIVE: u32 = 0x40;

/// The item a recipe creates through a create effect (24, 157, 59) in slot 0, the one slot the
/// icon (`0x595150`), the item link (`0x596290`) and the tooltip (`0x62e900`) read.
fn product_item(spell: u32, spells: &SpellCatalog) -> Option<u32> {
    let d = spells.get(spell)?;
    matches!(d.effects[0], 24 | 157 | 59)
        .then_some(d.effect_item_type[0])
        .filter(|&i| i != 0)
}

/// `GetTrainerServiceIcon` (`0x595150`): at a tradeskill trainer a recipe's product art, else the
/// wire spell's own icon. Deviation: the product's display comes from the item cache, since
/// `Item.dbc`, which the reference reads it from, is not loaded; an uncached template reads `None`
/// until its answer re-pushes the snapshot.
pub(super) fn service_icon(
    wire_spell: u32,
    trainer_type: u32,
    spells: &SpellCatalog,
    icons: Option<&ItemDisplays>,
    items: &Items,
    commands: &NetCommands,
) -> Option<String> {
    let wire = spells.get(wire_spell)?;
    if trainer_type == TRAINER_TYPE_TRADESKILL {
        if let Some(product) = product_item(wire_spell, spells) {
            return items
                .template(product, 0, commands)
                .map(|t| t.display_info_id)
                .and_then(|d| item_icon(icons, d));
        }
    }
    wire.icon.clone()
}

/// `GetTrainerServiceItemLink` (`0x596290`): a tradeskill recipe's cached product.
pub(super) fn service_item_link(
    wire_spell: u32,
    trainer_type: u32,
    spells: &SpellCatalog,
    items: &Items,
    commands: &NetCommands,
) -> Option<String> {
    if trainer_type != TRAINER_TYPE_TRADESKILL {
        return None;
    }
    let product = product_item(wire_spell, spells)?;
    let t = items.template(product, 0, commands)?;
    Some(crate::ui_items::item_link(product, &t.name, t.quality))
}

/// `GetTrainerServiceDescription`'s second leg (`0x5959d0`): a tradeskill recipe (`Attributes &
/// 0x20`) with no description of its own reads its cached product's description.
pub(super) fn service_item_description(
    wire_spell: u32,
    spells: &SpellCatalog,
    items: &Items,
    commands: &NetCommands,
) -> Option<String> {
    let d = spells.get(wire_spell)?;
    if d.attributes & SPELL_ATTR_IS_TRADESKILL == 0
        || d.description.as_deref().is_some_and(|t| !t.is_empty())
    {
        return None;
    }
    let product = product_item(wire_spell, spells)?;
    let t = items.template(product, 0, commands)?;
    Some(t.description.clone()).filter(|t| !t.is_empty())
}

/// `SetTrainerService` (`0x62e900`): the first learn slot (36 or 57) whose taught spell has a
/// record; a tradeskill recipe (`Attributes & 0x20`) that creates an item shows the item, any
/// other the taught spell, against the pet for a 57. No slot matched: the wire spell.
pub(super) fn service_tooltip(wire_spell: u32, spells: &SpellCatalog) -> TrainerTooltip {
    let wire_only = TrainerTooltip::Spell {
        spell_id: wire_spell,
        alt_caster: false,
    };
    let Some(wire) = spells.get(wire_spell) else {
        return wire_only;
    };
    for i in 0..3 {
        let effect = wire.effects[i];
        if effect != SPELL_EFFECT_LEARN_SPELL && effect != SPELL_EFFECT_LEARN_PET_SPELL {
            continue;
        }
        let trigger = wire.effect_trigger_spell[i];
        let Some(taught) = spells.get(trigger) else {
            continue;
        };
        if taught.attributes & SPELL_ATTR_IS_TRADESKILL != 0 {
            if let Some(item) = product_item(trigger, spells) {
                return TrainerTooltip::Item(item);
            }
        }
        return TrainerTooltip::Spell {
            spell_id: trigger,
            alt_caster: effect == SPELL_EFFECT_LEARN_PET_SPELL,
        };
    }
    wire_only
}

/// `AddServices`' group key (`0x596450`). A tradeskill trainer drops nothing: 3 for a passive, 1
/// for a skill step, else 2, labelled `TRADESKILL_SERVICE_*` (`0x594e80`). Elsewhere the taught
/// spell's skill line (`GetSkillLineFromService` `0x594ae0`), and 0 drops the service.
pub(super) fn service_group(
    wire_spell: u32,
    trainer_type: u32,
    spells: &SpellCatalog,
    skill_lines: Option<&SkillLineCatalog>,
    // The player's race and class, which the line lookup keys on.
    (race, class): (u8, u8),
    // The VM's own `GlobalStrings.lua`, for the header labels.
    get: &dyn Fn(&str) -> Option<String>,
) -> (u32, String) {
    if trainer_type == TRAINER_TYPE_TRADESKILL {
        let (key, label) = match spells.get(wire_spell) {
            Some(d) if d.attributes & SPELL_ATTR_PASSIVE != 0 => (3, "TRADESKILL_SERVICE_PASSIVE"),
            Some(d) if d.effects.contains(&SPELL_EFFECT_SKILL_STEP) => {
                (1, "TRADESKILL_SERVICE_STEP")
            }
            _ => (2, "TRADESKILL_SERVICE_LEARN"),
        };
        return (key, get(label).unwrap_or_default());
    }
    // The first learn slot on any target but the pet teaches; with none the spell stands for
    // itself.
    let taught = spells
        .learn_effects(wire_spell)
        .into_iter()
        .find_map(|e| match e {
            LearnEffect::Spell(t) => Some(t),
            _ => None,
        })
        .unwrap_or(wire_spell);
    let line = skill_lines
        .and_then(|c| c.spell_to_line(taught, race, class))
        .unwrap_or(0);
    if line == 0 {
        return (0, String::new());
    }
    let name = skill_lines
        .and_then(|c| c.line(line))
        .map(|l| l.name.clone())
        .unwrap_or_default();
    (line, name)
}

/// `GetTrainerServiceSkillLine` (`0x5952f0`): the first effect that names a line, a skill step's
/// own line or the taught (or plain) spell's ability line for the player's race and class.
pub(super) fn service_skill_line(
    wire_spell: u32,
    spells: &SpellCatalog,
    skill_lines: Option<&SkillLineCatalog>,
    (race, class): (u8, u8),
) -> Option<String> {
    let catalog = skill_lines?;
    spells.learn_effects(wire_spell).into_iter().find_map(|e| {
        let line = match e {
            LearnEffect::None => return None,
            LearnEffect::SkillStep { skill, .. } => skill,
            LearnEffect::Spell(t) => catalog.spell_to_line(t, race, class)?,
            LearnEffect::Other => catalog.spell_to_line(wire_spell, race, class)?,
        };
        catalog.line(line).map(|l| l.name.clone())
    })
}

/// `GetTrainerServiceStepIncrease` (`0x595c40`): `INCREASE_POTENTIAL` over the step's line and
/// five times its points; the step spell, which `0x6d12c0` zeroes, adds nothing.
pub(super) fn service_step_increase(
    wire_spell: u32,
    spells: &SpellCatalog,
    skill_lines: Option<&SkillLineCatalog>,
    get: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    let (skill, step) = spells
        .learn_effects(wire_spell)
        .into_iter()
        .find_map(|e| match e {
            LearnEffect::SkillStep { skill, step } => Some((skill, step)),
            _ => None,
        })?;
    if step == 0 {
        return None;
    }
    let name = skill_lines?.line(skill)?.name.clone();
    let format = get("INCREASE_POTENTIAL")?;
    Some(
        format
            .replacen("%s", &name, 1)
            .replacen("%d", &(5 * step).to_string(), 1),
    )
}

/// `GetTrainerServiceSkillReq` (`0x595470`): a gate needs both a line and a rank; it is met when
/// the player's slot for the line holds the rank, and no slot fails it.
pub(super) fn skill_req(
    req_skill: u32,
    req_skill_value: u32,
    skill_lines: Option<&SkillLineCatalog>,
    player: &PlayerView,
) -> Option<TrainerSkillReq> {
    if req_skill == 0 || req_skill_value == 0 {
        return None;
    }
    let line = skill_lines?.line(req_skill)?;
    let met = player
        .skill(req_skill)
        .is_some_and(|s| s.value_plus_perm >= req_skill_value as i32);
    Some(TrainerSkillReq {
        name: line.name.clone(),
        rank: req_skill_value,
        met,
    })
}

/// `GetTrainerServiceAbilityReq` (`0x5955e0`) per wire slot: "name (rank)", met when the player
/// knows it or a higher rank; a spell with no record reads nil and met.
pub(super) fn ability_reqs(
    slots: [u32; 3],
    spells: &SpellCatalog,
    skill_lines: Option<&SkillLineCatalog>,
    known: &BTreeSet<u32>,
) -> [Option<TrainerAbilityReq>; 3] {
    slots.map(|id| {
        (id as i32 > 0).then(|| match spells.get(id) {
            Some(d) => TrainerAbilityReq {
                name: Some(d.ranked_name()),
                met: skill_lines.map_or(known.contains(&id), |c| known_or_higher(id, known, c)),
            },
            None => TrainerAbilityReq {
                name: None,
                met: true,
            },
        })
    })
}

/// A wire `state` byte's category; anything but GREEN or GRAY, an unexpected value too, is gated.
pub(super) fn category(state: u8) -> TrainerServiceCategory {
    match state {
        trainer_spell_state::GRAY => TrainerServiceCategory::Used,
        trainer_spell_state::GREEN => TrainerServiceCategory::Available,
        _ => TrainerServiceCategory::Unavailable,
    }
}
