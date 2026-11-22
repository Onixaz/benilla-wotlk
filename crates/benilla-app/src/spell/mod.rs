//! The spell, as the reference's spell system: the cast ladder and commit ([`cast_send`], the cast
//! attempt and the cast send), the target bind ([`cast_target`], the target resolve and the target
//! bind), the party and raid relations its arms ask ([`group_relation`]), the requirement validator
//! ([`validator`]), the usable walk ([`usable`], the usability check), the targeting cursor
//! ([`targeting`]), the in-flight slot, the cooldowns, the talent modifiers and their packet
//! handlers ([`net`]).

use bevy::prelude::*;

use crate::char_select::ClientState;
use crate::ui_unit::UnitFeed;
use benilla_world::schedule::WorldStage;

mod bind_gates;
mod cast_send;
pub(crate) mod cast_target;
pub(crate) mod combo;
pub(crate) mod cooldowns;
mod dest_loc;
pub(crate) mod group_relation;
mod inflight;
pub(crate) mod learn_place;
mod mods;
pub(crate) mod multicast;
pub(crate) mod net;
mod range_check;
mod range_units;
mod rank_pick;
pub(crate) mod targeting;
pub(crate) mod text_caster;
pub(crate) mod usable;
pub(crate) mod validator;

// The one cast path: every caster takes [`CastLadder`] and commits through [`CastCommit`];
// `send_spell_cast` is private to `cast_send`, so no second send path can exist.
pub(crate) use cast_send::{
    attack_pick_step, cast_speed, run_ladder_rest, run_send_commit, run_send_tail, CastCommit,
    CastLadder, HeldCast, HeldForPick, LadderRest, SendCommit, SendTail, TargetedBind,
};
pub(crate) use cast_target::{
    AutoChecks, AutoDismount, AutoPosture, AutoSelfCast, SecureAbilityToggle,
};
pub(crate) use combo::ComboPoints;
pub(crate) use cooldowns::Cooldowns;
pub(crate) use inflight::{
    cancel_records, inflight, pet_setter_cancel, ActiveChannel, AutoRepeatActive, LocalMoveStart,
    PendingCast, PendingRecord, QueuedMeleeSpell, SelfCancel, SPELL_INTERRUPT_MOVEMENT,
};
pub(crate) use mods::{
    track_cost_inputs, SpellModifiers, OP_CAST_TIME, OP_COST, OP_GCD, OP_RADIUS,
};
#[cfg(test)]
pub(crate) use mods::{OP_COOLDOWN, OP_RANGE};
pub(crate) use range_check::{range_check, RangeFacts};
pub(crate) use range_units::{RangeUnits, DEFAULT_REACH};
// `TargetingWants` is exported for the ground reticle, which draws for the location word alone.
pub(crate) use targeting::{
    ground_cast_radius, CorpsePick, PicksSelf, ScriptCursor, SpellTargeting, TargetingWants,
};

/// A script cast's inputs, the cast tail the action bar uses, for the spellbook's and the stance
/// bar's calls ([`crate::script_calls`]).
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct ScriptCast<'w, 's> {
    pub(crate) targeting: cast_target::CastTargeting<'w, 's>,
    pub(crate) ladder: CastLadder<'w, 's>,
    /// Resolves a cast verb's unit token where the call takes effect.
    pub(crate) tokens: crate::ui_unit::UnitTokens<'w, 's>,
}

/// The local self-cancel's set: a reader of the in-flight state orders `.after(LocalCancel)` so a
/// cast ended by a move or jump drops its cast bar the same frame.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct LocalCancel;

pub(crate) struct SpellPlugin;

/// `SummonProperties.dbc`, the learn registrar's companion test (`0x8009b0`); empty without data.
#[derive(Resource, Default)]
pub(crate) struct SummonSlots(pub(crate) benilla_formats::SummonPropertiesCatalog);

fn load_summon_slots(mut commands: Commands, assets: Option<Res<benilla_assets::WorldAssets>>) {
    use benilla_assets::LockRecover;
    let Some(assets) = assets else { return };
    let loaded = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_summon_properties(&mut chain)
    };
    match loaded {
        Ok(catalog) => commands.insert_resource(SummonSlots(catalog)),
        Err(e) => {
            warn!("spell: SummonProperties.dbc failed — no learn reads as a companion: {e:#}")
        }
    }
}

impl Plugin for SpellPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PendingCast>()
            .init_resource::<QueuedMeleeSpell>()
            .init_resource::<ActiveChannel>()
            .init_resource::<LocalMoveStart>()
            .init_resource::<AutoRepeatActive>()
            .init_resource::<Cooldowns>()
            .init_resource::<ComboPoints>()
            .init_resource::<SpellModifiers>()
            .init_resource::<AutoSelfCast>()
            .init_resource::<AutoDismount>()
            .init_resource::<AutoPosture>()
            .init_resource::<SecureAbilityToggle>()
            .init_resource::<SpellTargeting>()
            .init_resource::<HeldForPick>()
            .init_resource::<targeting::EnchantConfirmItem>()
            .init_resource::<targeting::PicksSelf>()
            .init_resource::<targeting::CorpsePick>()
            .init_resource::<targeting::UnitPick>()
            .init_resource::<group_relation::GroupRoster>()
            .add_observer(cast_target::on_cvar)
            .add_systems(
                Update,
                (
                    // A move or jump from the last frame's input ends the cast before this frame's
                    // press: the move came first, so a cast pressed now is not the one it ends.
                    inflight::local_self_cancel
                        .after(WorldStage::Net)
                        .before(crate::ui_script::UiPress)
                        .in_set(LocalCancel),
                    // Before the press too, whose casts read the cost inputs.
                    (mods::track_class_family, mods::track_cost_inputs)
                        .chain()
                        .after(WorldStage::Net)
                        .before(crate::ui_script::UiPress)
                        .before(UnitFeed),
                    // The cursor's picks, after every verb and drain that arms or ends the
                    // targeting cursor and before the hover reads them in the input pass.
                    (
                        targeting::publish_picks_self,
                        targeting::publish_corpse_pick,
                        targeting::publish_unit_pick,
                    )
                        .after(crate::ui_script::UiDrain)
                        .before(WorldStage::Input),
                    // The party slots and raid roster the party and raid words read, after the
                    // packets and before the UI's first VM holder.
                    group_relation::publish_group_roster
                        .after(WorldStage::Net)
                        .before(crate::ui_script::UiPress),
                    // The item-target commit in the press pass after the clicks, outside the target chain, so its GCD reaches the frame's tick (`0x480410` before `0x495320`).
                    targeting::commit_item_cast_on_pick
                        .in_set(crate::ui_script::UiPress)
                        .after(crate::script_calls::apply_script_calls),
                ),
            )
            .add_systems(OnEnter(ClientState::InWorld), mods::clear_on_world_enter)
            .add_systems(OnEnter(ClientState::InWorld), combo::clear_combo_points)
            .add_systems(OnExit(ClientState::InWorld), inflight::clear_on_world_leave)
            .init_resource::<SummonSlots>()
            .init_resource::<multicast::MultiCastLists>()
            .add_systems(
                Startup,
                load_summon_slots.after(benilla_assets::AssetSet::Open),
            );
        net::register(app);
        dest_loc::register(app);
        combo::register(app);
    }
}

/// The player's skill in `spell_id`'s own line, the input of every per-level term: the reference
/// hops spell to SkillLineAbility line, then reads that line's `PLAYER_SKILL_INFO` slot. Every
/// missing input reads 0, like the reference's null paths.
pub(crate) fn spell_skill_value(
    me: Option<&crate::net::ObjectStore>,
    skill_lines: Option<&benilla_formats::SkillLineCatalog>,
    spell_id: u32,
) -> u32 {
    let (race, class) = race_class(me);
    let Some(line) = skill_lines.and_then(|c| c.spell_to_line(spell_id, race, class)) else {
        return 0;
    };
    skill_line_value(me, line)
}

/// The player's race and class bytes, which the ability lookup (`0x812410`) keys on; 0/0 before the
/// descriptor.
pub(crate) fn race_class(me: Option<&crate::net::ObjectStore>) -> (u8, u8) {
    me.map_or((0, 0), |s| {
        (s.0.unit_race().unwrap_or(0), s.0.unit_class().unwrap_or(0))
    })
}

/// The player's skill in `line`, [`spell_skill_value`]'s second half: 0 without a player or a
/// slot for it.
pub(crate) fn skill_line_value(me: Option<&crate::net::ObjectStore>, line: u32) -> u32 {
    let Some(store) = me else { return 0 };
    line_skill_value(
        (0..benilla_protocol::messages::PLAYER_SKILL_SLOTS)
            .filter_map(|slot| store.0.player_skill(slot)),
        line,
    )
}

/// The reference's sum on the line's first slot: the value, plus the bonus word's high half read
/// unsigned when the value is positive, plus its low half read signed, floored at 0.
fn line_skill_value(
    slots: impl Iterator<Item = benilla_protocol::messages::PlayerSkillSlot>,
    line: u32,
) -> u32 {
    for s in slots {
        if u32::from(s.skill_id) == line {
            let mut v = i32::from(s.value);
            if v > 0 {
                v += i32::from(s.perm_bonus as u16);
            }
            return (v + i32::from(s.temp_bonus)).max(0) as u32;
        }
    }
    0
}

/// Each skill slot's line and effective value, all [`spell_skill_value`] reads: a text built
/// against one snapshot is rebuilt when the next differs.
pub(crate) type SkillSnapshot =
    [(u16, u32); benilla_protocol::messages::PLAYER_SKILL_SLOTS as usize];

/// The [`SkillSnapshot`] of the player; all zeros without one.
pub(crate) fn skill_snapshot(me: Option<&crate::net::ObjectStore>) -> SkillSnapshot {
    std::array::from_fn(|slot| {
        me.and_then(|s| s.0.player_skill(slot as u8))
            .map_or((0, 0), |s| {
                (
                    s.skill_id,
                    line_skill_value(std::iter::once(s), s.skill_id.into()),
                )
            })
    })
}

#[cfg(test)]
mod tests {
    use benilla_protocol::messages::PlayerSkillSlot;

    fn slot(value: u16, temp_bonus: i16, perm_bonus: i16) -> PlayerSkillSlot {
        PlayerSkillSlot {
            skill_id: 43,
            step: 0,
            value,
            max: 300,
            temp_bonus,
            perm_bonus,
        }
    }

    /// The high bonus half counts only over a positive value, and unsigned; the low half is
    /// signed and always counts; the sum floors at 0.
    #[test]
    fn the_skill_value_reads_its_bonus_halves_as_the_reference_does() {
        let value = |s: PlayerSkillSlot| super::line_skill_value(std::iter::once(s), 43);
        assert_eq!(value(slot(100, 0, 5)), 105);
        assert_eq!(value(slot(0, 0, 5)), 0);
        assert_eq!(value(slot(0, 7, 5)), 7);
        assert_eq!(value(slot(100, -20, 0)), 80);
        assert_eq!(value(slot(10, -20, 0)), 0);
        assert_eq!(value(slot(1, 0, -1)), 65536);
        assert_eq!(
            super::line_skill_value(std::iter::once(slot(100, 0, 0)), 44),
            0
        );
    }
}
