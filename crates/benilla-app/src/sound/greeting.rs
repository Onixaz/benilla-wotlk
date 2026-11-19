//! NPC greetings: the select greeting and the interaction-window hello and goodbye.
//!
//! Both read `CreatureDisplayInfo.field[11]` NPCSoundID → `NPCSounds {hello +0x4, goodbye +0x8,
//! pissed +0xc}` → a `SoundEntries` kit, played 3D at the NPC, and both respect the unit's channel
//! latch `[unit+0xb1c]`: a unit refuses a new line while its last one sounds
//! ([`kit::source_playing`]).
//!
//! ## The select greeting (left-click)
//!
//! Left-clicking a unit ([`crate::target::select_on_click`], before the target is set) plays the
//! cycling welcome ([`NpcGreetingRequest`]). A counter per NPC ([`GreetSeq`], reset when the
//! greeted NPC changes) steps ([`greet_line`]): five hello takes, then the pissed kit's takes in
//! order, then a wrap.
//!
//! ## The window hello and goodbye (right-click)
//!
//! The vocal belongs to the interact target, not to any one window. Setting the target (`0x51fce0`)
//! plays the NPC sound's hello (`0x720860`, `+0x4`, free-picked take) when it changes to a
//! different non-player unit. Clearing it (`0x518d50`) plays the goodbye (`+0x8`) only when it
//! clears to nothing: the set clears the old target with "the new guid is non-zero", so a swap to
//! another target plays the new hello with no goodbye for the old one. Every NPC window opener sets
//! the target, the taxi map's `SMSG_SHOWTAXINODES` open (`0x5932e0`) among them, and the NPC sound
//! itself stays silent while the player is dead or a ghost or the NPC reads dead
//! ([`npc_sound_silenced`]). The set and the clear hand their line here as an
//! [`InteractCue::Sound`].

use bevy::prelude::*;

use benilla_formats::NpcGreetingCatalog;
use benilla_protocol::EntityKind;

use crate::net::{GuidIndex, NetEntity, ObjectStore, SelfPlayer};
use crate::ui_session::InteractCue;
use benilla_assets::{AssetSet, LockRecover, WorldAssets};
use benilla_protocol::field::UNIT_DYNFLAG_DEAD;
use benilla_protocol::ObjectFields;
use benilla_world::schedule::WorldStage;
use benilla_world::view::WorldCamera;

use super::kit::{self, play_kit_ext, KitRef, Latch, PlayExtras, SoundCategory, SoundKits};
use super::{SoundConfig, SoundOutput};

/// Hello takes before the select greeter steps into the pissed kit.
const HELLO_TAKES: usize = 5;

/// Plays an NPC's select greeting, sent by [`crate::target::select_on_click`].
#[derive(Message, Clone, Copy)]
pub(crate) struct NpcGreetingRequest {
    pub(crate) npc: Entity,
}

/// The display→greeting catalog (`CreatureDisplayInfo.NPCSoundID` → `NPCSounds`).
#[derive(Resource)]
struct NpcGreetings(NpcGreetingCatalog);

/// The select greeting's counter, keyed by the tracked NPC and reset when it changes; it advances
/// only on a play that started.
#[derive(Default)]
struct GreetSeq {
    tracked: Option<Entity>,
    seq: usize,
}

/// The line and take a select play produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GreetLine {
    /// HELLO (`+0x4`), free-pick take.
    Hello,
    /// PISSED (`+0xc`), take `variation`.
    Pissed(usize),
}

/// [`HELLO_TAKES`] hello free-picks, then the pissed kit's takes in order, then a hello that resets
/// the counter. Returns the line and the next counter.
fn greet_line(seq: usize, pissed_variations: usize) -> (GreetLine, usize) {
    if seq < HELLO_TAKES {
        (GreetLine::Hello, seq + 1)
    } else {
        let p = seq - HELLO_TAKES;
        if p < pissed_variations {
            (GreetLine::Pissed(p), seq + 1)
        } else {
            // edi >= count: hello, variation -1, counter reset.
            (GreetLine::Hello, 0)
        }
    }
}

fn load_npc_greetings(mut commands: Commands, assets: Option<Res<WorldAssets>>) {
    let Some(assets) = assets else { return };
    let loaded = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_npc_greeting_catalog(&mut chain)
    };
    match loaded {
        Ok(cat) => {
            info!("sound: {} NPC greeting rows", cat.len());
            commands.insert_resource(NpcGreetings(cat));
        }
        Err(e) => warn!("sound: NPC greetings failed to load: {e:#}"),
    }
}

/// A live non-player unit (type-mask bit 3 set, bit 4 clear) whose display has an NPCSoundID row.
fn resolve(
    npc: Entity,
    units: &Query<(&NetEntity, &Transform, Option<&ObjectStore>)>,
    greetings: &NpcGreetings,
) -> Option<(benilla_formats::NpcGreeting, Vec3)> {
    let (net, transform, store) = units.get(npc).ok()?;
    if !matches!(net.kind, EntityKind::Unit) {
        return None;
    }
    if store.is_some_and(|s| s.0.unit_is_dead()) {
        return None;
    }
    let row = *greetings.0.for_display(net.display_id?)?;
    Some((row, transform.translation))
}

/// Plays one line, kit 0 being none and `None` a free-picked take, gated on the unit's channel
/// latch.
fn play_line(
    npc: Entity,
    kit_id: u32,
    variant: Option<usize>,
    pos: Vec3,
    listener: Vec3,
    kits: &mut SoundKits,
    assets: &WorldAssets,
    out: &mut SoundOutput,
    config: &SoundConfig,
) {
    if kit_id == 0 || kit::source_playing(out, npc) {
        return;
    }
    if let Err(e) = play_kit_ext(
        kits,
        assets,
        out,
        config,
        listener,
        KitRef::Id(kit_id),
        Some(pos),
        SoundCategory::Sfx,
        PlayExtras {
            variant,
            source: Some(npc),
            // The one play that takes `[unit+0xb1c]`; the unit's other sounds do not.
            latch: Latch::Greeting,
            ..default()
        },
    ) {
        warn!("npc vocal (kit {kit_id}): {e:#}");
    }
}

/// The select greeting.
fn play_select_greetings(
    mut reqs: MessageReader<NpcGreetingRequest>,
    mut seq: Local<GreetSeq>,
    units: Query<(&NetEntity, &Transform, Option<&ObjectStore>)>,
    greetings: Option<Res<NpcGreetings>>,
    kits: Option<ResMut<SoundKits>>,
    assets: Option<Res<WorldAssets>>,
    mut out: NonSendMut<SoundOutput>,
    config: Res<SoundConfig>,
    cam: Query<&Transform, (With<WorldCamera>, Without<NetEntity>)>,
) {
    if reqs.is_empty() {
        return;
    }
    let (Some(greetings), Some(mut kits), Some(assets)) = (greetings, kits, assets) else {
        reqs.clear();
        return;
    };
    let listener = cam.single().map(|t| t.translation).unwrap_or(Vec3::ZERO);

    for req in reqs.read() {
        let Some((row, pos)) = resolve(req.npc, &units, &greetings) else {
            continue;
        };
        // A different NPC resets the sequence counter.
        if seq.tracked != Some(req.npc) {
            *seq = GreetSeq {
                tracked: Some(req.npc),
                seq: 0,
            };
        }
        // The latch gate precedes the counter: a re-click while the line sounds neither plays nor
        // advances.
        if kit::source_playing(&out, req.npc) {
            continue;
        }
        let (line, next) = greet_line(seq.seq, kits.variations(row.pissed));
        let (kit_id, variant) = match line {
            GreetLine::Hello => (row.hello, None),
            GreetLine::Pissed(p) => (row.pissed, Some(p)),
        };
        if kit_id == 0 {
            continue;
        }
        // Commit the counter only if the play started; the channel was idle, so a live one now
        // means it did.
        play_line(
            req.npc, kit_id, variant, pos, listener, &mut kits, &assets, &mut out, &config,
        );
        if kit::source_playing(&out, req.npc) {
            seq.seq = next;
        }
    }
}

/// The NPC sound's own gates (`0x720860`), past its player check and the channel latch
/// [`play_line`] keeps: no active player, a dead or ghost one, or an NPC whose health reads 0 or
/// that carries `UNIT_DYNFLAG_DEAD` plays nothing. An NPC with no fields yet is not gated.
fn npc_sound_silenced(player: Option<&ObjectFields>, npc: Option<&ObjectFields>) -> bool {
    player.is_none_or(|p| p.is_dead_or_ghost())
        || npc.is_some_and(|n| {
            n.unit_health().unwrap_or(0) == 0 || n.unit_dynamic_flags() & UNIT_DYNFLAG_DEAD != 0
        })
}

/// The window hello and goodbye, as the interact target's set and clear call them (`0x51fda4`, `0x518da5`).
#[allow(clippy::too_many_arguments)] // one Bevy system's full input set
fn play_window_greetings(
    mut cues: MessageReader<InteractCue>,
    index: Res<GuidIndex>,
    units: Query<(&NetEntity, &Transform, Option<&ObjectStore>)>,
    greetings: Option<Res<NpcGreetings>>,
    kits: Option<ResMut<SoundKits>>,
    assets: Option<Res<WorldAssets>>,
    mut out: NonSendMut<SoundOutput>,
    config: Res<SoundConfig>,
    cam: Query<&Transform, (With<WorldCamera>, Without<NetEntity>)>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
) {
    let lines: Vec<(u64, bool)> = cues
        .read()
        .filter_map(|c| match *c {
            InteractCue::Sound { guid, hello } => Some((guid, hello)),
            _ => None,
        })
        .collect();
    let (Some(greetings), Some(mut kits), Some(assets)) = (greetings, kits, assets) else {
        return;
    };
    let listener = cam.single().map(|t| t.translation).unwrap_or(Vec3::ZERO);
    for (guid, hello) in lines {
        // A despawned NPC resolves nothing: the reference's sound needs the held unit (`0x518d8b`).
        let Some(&npc) = index.0.get(&guid) else {
            continue;
        };
        let Some((row, pos)) = resolve(npc, &units, &greetings) else {
            continue;
        };
        let npc_fields = units.get(npc).ok().and_then(|(_, _, s)| s).map(|s| &s.0);
        if npc_sound_silenced(self_q.single().ok().map(|s| &s.0), npc_fields) {
            continue;
        }
        let kit_id = if hello { row.hello } else { row.goodbye };
        // Both window lines free-pick their take (`variation = -1`).
        play_line(
            npc, kit_id, None, pos, listener, &mut kits, &assets, &mut out, &config,
        );
    }
}

/// A despawning unit stops its greeting.
fn stop_on_despawn(mut removed: RemovedComponents<NetEntity>, mut out: NonSendMut<SoundOutput>) {
    for entity in removed.read() {
        kit::stop_source(&mut out, entity);
    }
}

/// Registration hook for [`super::SoundPlugin`].
pub(super) fn plugin(app: &mut App) {
    app.add_message::<NpcGreetingRequest>()
        .add_systems(Startup, load_npc_greetings.after(AssetSet::Open))
        .add_systems(
            Update,
            (
                play_select_greetings,
                play_window_greetings,
                stop_on_despawn,
            )
                .in_set(WorldStage::Present),
        );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_sequence_cycles_5_hello_then_pissed_then_wraps() {
        for s in 0..5 {
            assert_eq!(greet_line(s, 3), (GreetLine::Hello, s + 1));
        }
        assert_eq!(greet_line(5, 3), (GreetLine::Pissed(0), 6));
        assert_eq!(greet_line(6, 3), (GreetLine::Pissed(1), 7));
        assert_eq!(greet_line(7, 3), (GreetLine::Pissed(2), 8));
        assert_eq!(greet_line(8, 3), (GreetLine::Hello, 0));
        // An NPC with no pissed takes wraps straight after the five hellos.
        assert_eq!(greet_line(5, 0), (GreetLine::Hello, 0));
    }

    #[test]
    fn the_npc_sound_is_silent_for_a_dead_player_or_a_dead_npc() {
        use benilla_protocol::field::{
            FIELD_PLAYER_FLAGS, FIELD_UNIT_DYNAMIC_FLAGS, FIELD_UNIT_HEALTH, FIELD_UNIT_MAXHEALTH,
        };
        let unit = |health: u32, extra: &[(u16, u32)]| {
            let mut pairs = vec![(FIELD_UNIT_HEALTH, health), (FIELD_UNIT_MAXHEALTH, 100)];
            pairs.extend_from_slice(extra);
            ObjectFields::from_pairs(&pairs)
        };
        let (alive, dead) = (unit(100, &[]), unit(0, &[]));
        let ghost = unit(1, &[(FIELD_PLAYER_FLAGS, 0x10)]);
        let feigning = unit(100, &[(FIELD_UNIT_DYNAMIC_FLAGS, UNIT_DYNFLAG_DEAD)]);

        assert!(!npc_sound_silenced(Some(&alive), Some(&alive)));
        assert!(
            !npc_sound_silenced(Some(&alive), None),
            "no fields yet: not gated"
        );
        assert!(npc_sound_silenced(None, Some(&alive)), "no active player");
        assert!(npc_sound_silenced(Some(&dead), Some(&alive)));
        assert!(npc_sound_silenced(Some(&ghost), Some(&alive)));
        assert!(npc_sound_silenced(Some(&alive), Some(&dead)));
        assert!(npc_sound_silenced(Some(&alive), Some(&feigning)));
    }
}
