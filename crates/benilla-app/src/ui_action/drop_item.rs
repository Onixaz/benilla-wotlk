//! `DropItemOnUnit`: a held item dropped on a unit, which in 1.12 feeds the pet. On the pet the
//! validator casts the learned Feed Pet with no cast item, so the wire is `CMSG_CAST_SPELL` with
//! `TARGET_FLAG_ITEM` and the food's guid, never the pet's. On a player the reference starts a
//! trade or fills a trade slot; that leg is not built and refuses. Every refusal is silent and
//! keeps the payload (`ClearCursor` runs on success only), and the server alone judges range.

use bevy::prelude::*;

use benilla_ui::script::CursorPayload;
use benilla_world::interact::WorldClick;

use crate::spell::{CastCommit, TargetedBind};
use crate::target::WorldDropClick;

/// The two payload legs of a press, each with the step that fires what it raised: the world
/// click's select reads the cursor they leave.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct DropSet;

/// The feed validator's three gates, in order: is this a pet I can feed? The food is not checked
/// here; the server judges the diet (`Spell.cpp:5912`, `SPELL_FAILED_WRONG_PET_FOOD`).
fn feedable_pet(
    pet: &benilla_protocol::ObjectFields,
    self_guid: u64,
    feed_pet_known: bool,
) -> bool {
    // 1: `UNIT_CREATED_BY_SPELL` is set, so something summoned it.
    pet.unit_created_by_spell().is_some()
        // 2: `UNIT_FIELD_CREATEDBY` is my guid.
        && pet.unit_created_by() == Some(self_guid)
        // 3: the Feed Pet latch: an imp passes 1 and 2, but only hunters learn it.
        && feed_pet_known
}

/// Runs the pet leg for both entries into the feed validator (`DropItemOnUnit("pet")` and a held item's left click on the pet's model), each token its own press.
pub(crate) fn drop_item_on_unit(world: &mut World) {
    let clicked_pet = world.run_system_cached(clicked_pet).unwrap_or(false);
    press_dropped_tokens(world);
    if clicked_pet {
        crate::script_calls::press_step(world, feed_leg, "pet".to_string());
    }
}

/// The tokens `DropItemOnUnit` queued, each its own press: inside the verb's call where the `World` is lent (`0x51bf26`), else in the press pass.
pub(crate) fn press_dropped_tokens(world: &mut World) {
    let tokens = crate::ui_script::safe_world::lend(world, |s| s.take_drop_item_on_unit());
    for token in tokens.unwrap_or_default() {
        crate::script_calls::press_step(world, feed_leg, token);
    }
}

/// The world entry: a click whose press-time pick is the pet. That is the pick the click selects from; the live hover can differ after a drag.
fn clicked_pet(
    press: Res<crate::target::PressPick>,
    pet_bar: Option<Res<crate::ui_pet::PetBar>>,
    mut clicks: MessageReader<benilla_world::interact::WorldClick>,
) -> bool {
    let pet_guid = pet_bar.as_ref().map_or(0, |b| b.spells.pet_guid);
    clicks.read().count() > 0 && pet_guid != 0 && press.hovered.guid == Some(pet_guid)
}

/// One `DropItemOnUnit` token.
#[allow(clippy::too_many_arguments)]
fn feed_leg(
    In(token): In<String>,
    script: NonSend<benilla_ui::script::UiScript>,
    mut ui: ResMut<crate::ui_script::live::UiEvents>,
    self_q: Query<(&crate::net::Guid, &crate::net::ObjectStore), With<crate::net::SelfPlayer>>,
    stores: Query<&crate::net::ObjectStore>,
    index: Res<crate::net::GuidIndex>,
    pet_bar: Option<Res<crate::ui_pet::PetBar>>,
    learned: Res<super::LearnedAbilities>,
    mut ladder: crate::spell::CastLadder,
) {
    let pet_guid = pet_bar.as_ref().map_or(0, |b| b.spells.pet_guid);
    let Some((self_guid, self_store)) = self_q.iter().next() else {
        return;
    };
    // Only the "pet" token is the pet; any other unit takes the unbuilt trade leg.
    if token != "pet" {
        debug!("DropItemOnUnit({token}) — only the pet leg is modelled; refused, payload kept");
        return;
    }
    let Some(pet_fields) = (pet_guid != 0)
        .then(|| index.0.get(&pet_guid))
        .flatten()
        .and_then(|&e| stores.get(e).ok())
    else {
        return; // no pet, or not streamed in: silent, payload kept
    };
    let Some(spell_id) = learned.feed_pet else {
        return; // gate 3 fails outright: we never learned Feed Pet
    };
    if !feedable_pet(&pet_fields.0, self_guid.0, true) {
        debug!("DropItemOnUnit(\"pet\") — not a pet I summoned; refused, payload kept");
        return;
    }
    // The food: the held item's live guid. Only an item payload can be dropped on a unit.
    let Some(CursorPayload::Item(held)) = script.cursor_payload() else {
        return;
    };
    let slot0 = u8::try_from(held.slot.saturating_sub(1)).unwrap_or(0);
    let Some(item_guid) =
        crate::ui_items::slot_guid(&self_store.0, held.bag, slot0, &ladder.objects)
    else {
        return; // the slot emptied: silent, as every refusal here is
    };
    debug!("DropItemOnUnit(\"pet\") — Feed Pet {spell_id} at item {item_guid:#x}");
    ladder.commit_targeted(spell_id, CastCommit::Spell, TargetedBind::Item(item_guid));
    // The send's cast-bar edges, fired at its step (`0x80b556`).
    ladder.press.defer(&mut ui);
    // `ClearCursor(1,1)`, on success only, after the send's events.
    ui.world(|world| {
        crate::ui_script::safe_world::lend(world, |s| s.clear_cursor_payload());
    });
}

/// The left click's payload legs on the press pick, at the settled click (`0x4f7880`).
pub(crate) fn drop_on_world_click(world: &mut World) {
    let pick = world.run_system_cached(settled_click).ok().flatten();
    if let Some(pick) = pick {
        crate::ui_script::safe_world::lend(world, |s| s.world_drop_click(pick));
    }
}

/// The press pick of a settled click, unless a pending location cast takes the terrain click first (`0x52737a` -> `0x80c340`, the item's leg at `0x52738e`).
fn settled_click(
    press: Res<crate::target::PressPick>,
    ground: Option<Res<crate::spell::SpellTargeting>>,
    mut clicks: MessageReader<WorldClick>,
    mut drops: MessageReader<WorldDropClick>,
) -> Option<benilla_ui::script::WorldPick> {
    let clicked = clicks.read().count() + drops.read().count() > 0;
    let taken = ground
        .as_deref()
        .is_some_and(|g| crate::spell::targeting::location_cast_takes(g, &press.occlusion));
    (clicked && !taken).then(|| press.world_pick())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The feed validator's three gates, each failed alone.
    #[test]
    fn the_feed_gates_are_ownership_and_provenance() {
        use benilla_protocol::messages::ObjectFields;
        const ME: u64 = 0xDEAD_BEEF;
        // Field 146 is `UNIT_CREATED_BY_SPELL`; 14/15 are `UNIT_FIELD_CREATEDBY`'s guid pair.
        let pet = |created_by_spell: Option<u32>, created_by: Option<u64>| {
            let mut pairs: Vec<(u16, u32)> = Vec::new();
            if let Some(s) = created_by_spell {
                pairs.push((benilla_protocol::field::FIELD_UNIT_CREATED_BY_SPELL, s));
            }
            if let Some(g) = created_by {
                pairs.push((benilla_protocol::field::FIELD_UNIT_CREATEDBY, g as u32));
                pairs.push((
                    benilla_protocol::field::FIELD_UNIT_CREATEDBY + 1,
                    (g >> 32) as u32,
                ));
            }
            ObjectFields::from_pairs(&pairs)
        };
        let mine = pet(Some(883), Some(ME));
        assert!(feedable_pet(&mine, ME, true), "my own summoned pet feeds");

        // 1: not summoned, though it names me.
        assert!(!feedable_pet(&pet(None, Some(ME)), ME, true));
        // 2: summoned by someone else.
        assert!(!feedable_pet(&pet(Some(883), Some(1)), ME, true));
        // 3: a warlock's imp passes both field gates, but its owner never learns Feed Pet.
        assert!(!feedable_pet(&mine, ME, false));
    }
}
