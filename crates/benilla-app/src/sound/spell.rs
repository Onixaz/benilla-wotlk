//! Spell-visual kit sounds: [`SpellKitSound`] (the kit's `SoundEntries.dbc` id, `SpellVisualKit`
//! column 15) played at the casting unit, split as the reference's looping test splits it. A plain
//! kit is a positioned one-shot; a looping kit (Fireball's buildup 702, Arcane Missiles' hum 3136)
//! is a channel tracked to the caster, reaped by [`SpellKitSound::StopHold`] as the reference kills
//! the effect's sound.

use bevy::ecs::entity::EntityHashMap;
use bevy::prelude::*;

use crate::creature_anim::{SpellKitSound, UnitEffectClear};
use crate::net::NetEntity;
use benilla_assets::WorldAssets;
use benilla_world::schedule::WorldStage;

use super::kit::{
    kit_looping, play_kit, play_kit_ext, KitRef, PlayExtras, SoundCategory, SoundKits,
};
use super::{AudioListener, SoundConfig, SoundOutput};

/// Each unit's looping kit plays as `(kit, spell)`, the effects an effect-list clear stops.
#[derive(Default)]
struct KitLoops(EntityHashMap<Vec<(u32, u32)>>);

impl KitLoops {
    fn drop_kit(&mut self, entity: Entity, kit_sound: u32) {
        if let Some(l) = self.0.get_mut(&entity) {
            l.retain(|&(k, _)| k != kit_sound);
        }
    }

    /// The loops an effect-list clear ends, dropped from the list for the caller to stop.
    fn clear(
        &mut self,
        clear: &UnitEffectClear,
        spells: Option<&crate::ui_action::Spells>,
    ) -> Vec<u32> {
        let Some(l) = self.0.get_mut(&clear.entity) else {
            return Vec::new();
        };
        let mut ended = Vec::new();
        l.retain(|&(kit_sound, spell_id)| {
            let ends = clear.mode.ends(spells, spell_id);
            if ends {
                ended.push(kit_sound);
            }
            !ends
        });
        ended
    }
}

#[allow(clippy::too_many_arguments)] // one Bevy system's full input set
fn route_spell_kit_sounds(
    mut events: MessageReader<SpellKitSound>,
    (mut list_clears, spells): (
        MessageReader<UnitEffectClear>,
        Option<Res<crate::ui_action::Spells>>,
    ),
    transforms: Query<&Transform, Without<Camera3d>>,
    kits: Option<ResMut<SoundKits>>,
    assets: Option<Res<WorldAssets>>,
    mut out: NonSendMut<SoundOutput>,
    config: Res<SoundConfig>,
    listener: Res<AudioListener>,
    // Each unit's live hold-loop kit, so StopHold reaps only that kit's channels.
    mut hold_loops: Local<EntityHashMap<u32>>,
    mut loops: Local<KitLoops>,
    mut despawned: RemovedComponents<NetEntity>,
) {
    for entity in despawned.read() {
        hold_loops.remove(&entity); // the channel dies with the source-channel despawn reaper
        loops.0.remove(&entity);
    }
    if !events.is_empty() {
        if let (Some(mut kits), Some(assets)) = (kits, assets) {
            route_kit_events(
                &mut events,
                &transforms,
                &mut kits,
                &assets,
                &mut out,
                &config,
                listener.pos,
                (&mut hold_loops, &mut loops),
            );
        }
        events.clear();
    }
    // The effect-list clear stops each looping effect's sound it ends, a one-shot rings out
    // (`0x6f8843`).
    for c in list_clears.read() {
        for kit_sound in loops.clear(c, spells.as_deref()) {
            if hold_loops.get(&c.entity) == Some(&kit_sound) {
                hold_loops.remove(&c.entity);
            }
            super::kit::stop_source_kit(&mut out, c.entity, kit_sound);
        }
    }
}

/// Ring and stop this frame's kit sounds.
#[allow(clippy::too_many_arguments)] // the router's full sound state
fn route_kit_events(
    events: &mut MessageReader<SpellKitSound>,
    transforms: &Query<&Transform, Without<Camera3d>>,
    kits: &mut SoundKits,
    assets: &WorldAssets,
    out: &mut SoundOutput,
    config: &SoundConfig,
    listener: Vec3,
    (hold_loops, loops): (&mut EntityHashMap<u32>, &mut KitLoops),
) {
    // One ring per unit and kit per frame: the impact flash and the aura watcher's add edge can
    // both play a state kit in one packet burst. Plays in different frames both ring.
    let mut played_now: Vec<(Entity, u32)> = Vec::new();
    for ev in events.read() {
        match *ev {
            SpellKitSound::Play {
                entity,
                kit_sound,
                spell_id,
            } => {
                if played_now.contains(&(entity, kit_sound)) {
                    continue;
                }
                played_now.push((entity, kit_sound));
                let pos = transforms.get(entity).map(|t| t.translation).ok();
                let looping = kit_looping(kits, kit_sound);
                // The line a headless probe greps to prove a spell's sound fired.
                debug!("spell kit sound {kit_sound} on {entity:?} (looping {looping})");
                let played = if looping {
                    play_kit_ext(
                        kits,
                        assets,
                        out,
                        config,
                        listener,
                        KitRef::Id(kit_sound),
                        pos,
                        SoundCategory::Sfx,
                        PlayExtras {
                            source: Some(entity),
                            ..default()
                        },
                    )
                } else {
                    play_kit(
                        kits,
                        assets,
                        out,
                        config,
                        listener,
                        KitRef::Id(kit_sound),
                        pos,
                        SoundCategory::Sfx,
                    )
                };
                match played {
                    Ok(_) if looping => {
                        hold_loops.insert(entity, kit_sound);
                        let l = loops.0.entry(entity).or_default();
                        if !l.contains(&(kit_sound, spell_id)) {
                            l.push((kit_sound, spell_id));
                        }
                    }
                    Ok(_) => {}
                    Err(e) => warn!("spell kit sound {kit_sound}: {e:#}"),
                }
            }
            SpellKitSound::PlayAt { pos, kit_sound } => {
                // A bare positional one-shot with no owner or dedup: its one caller is a
                // missile's ground arrival.
                debug!("spell kit sound {kit_sound} at {pos:?}");
                if let Err(e) = play_kit(
                    kits,
                    assets,
                    out,
                    config,
                    listener,
                    KitRef::Id(kit_sound),
                    Some(pos),
                    SoundCategory::Sfx,
                ) {
                    warn!("spell kit sound {kit_sound}: {e:#}");
                }
            }
            SpellKitSound::StopHold { entity } => {
                if let Some(kit_sound) = hold_loops.remove(&entity) {
                    loops.drop_kit(entity, kit_sound);
                    super::kit::stop_source_kit(out, entity, kit_sound);
                }
            }
            SpellKitSound::StopKit { entity, kit_sound } => {
                // An aura-drop reap: stop this kit's channels; an unrelated hold loop survives.
                if hold_loops.get(&entity) == Some(&kit_sound) {
                    hold_loops.remove(&entity);
                }
                loops.drop_kit(entity, kit_sound);
                super::kit::stop_source_kit(out, entity, kit_sound);
            }
        }
    }
}

/// Registration hook for [`super::SoundPlugin`].
pub(super) fn plugin(app: &mut App) {
    app.add_systems(Update, route_spell_kit_sounds.in_set(WorldStage::Present));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::creature_anim::EffectListClear;

    /// A death stops each looping effect's sound but a death-persistent spell's; a release stops
    /// them all; another unit's loops stay (`0x6f8843`).
    #[test]
    fn a_list_clear_stops_the_units_looping_effects() {
        const SOULSTONE: u32 = 20707;
        let spells = EffectListClear::death_persistent_for_tests(SOULSTONE);
        let id = |n| Entity::from_raw_u32(n).expect("a test entity id");
        let (unit, other) = (id(1), id(2));
        let mut loops = KitLoops::default();
        loops.0.insert(unit, vec![(702, 133), (3136, SOULSTONE)]);
        loops.0.insert(other, vec![(702, 133)]);
        let clear = |entity, mode| UnitEffectClear { entity, mode };
        assert_eq!(
            loops.clear(&clear(unit, EffectListClear::Death), Some(&spells)),
            vec![702]
        );
        assert_eq!(
            loops.clear(&clear(unit, EffectListClear::Release), Some(&spells)),
            vec![3136]
        );
        assert_eq!(loops.0[&other], vec![(702, 133)]);
    }
}
