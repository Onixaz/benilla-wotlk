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
//! ([`npc_sound_silenced`]). Here the interact target is [`InteractNpc`], the union of every
//! NPC-bound session that also drives the `"npc"` unit. A flight master with no gossip flag (most
//! of Northrend's) opens its taxi map directly, with no gossip session in front of it, so only the
//! taxi session carries the vocal.

use bevy::prelude::*;

use benilla_formats::NpcGreetingCatalog;
use benilla_protocol::EntityKind;

use crate::net::{GuidIndex, NetEntity, ObjectStore, SelfPlayer};
use crate::ui_session::InteractNpc;
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

/// The vocal an interact-target change produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WindowVocal {
    Hello(u64),
    Goodbye(u64),
    None,
}

/// The interact target's vocal (`0x51fce0`): a different target plays its hello with no goodbye
/// for the old (`0x518d50` told the new guid is non-zero); clearing to nothing plays the goodbye.
fn window_vocal(prev: Option<u64>, active: Option<u64>) -> WindowVocal {
    match (prev, active) {
        (p, a) if p == a => WindowVocal::None,
        (_, Some(new)) => WindowVocal::Hello(new),
        (Some(old), None) => WindowVocal::Goodbye(old),
        (None, None) => WindowVocal::None,
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

/// The window hello and goodbye, from the interact target's change.
fn play_window_greetings(
    interact: Res<InteractNpc>,
    index: Res<GuidIndex>,
    mut prev_active: Local<Option<u64>>,
    units: Query<(&NetEntity, &Transform, Option<&ObjectStore>)>,
    greetings: Option<Res<NpcGreetings>>,
    kits: Option<ResMut<SoundKits>>,
    assets: Option<Res<WorldAssets>>,
    mut out: NonSendMut<SoundOutput>,
    config: Res<SoundConfig>,
    cam: Query<&Transform, (With<WorldCamera>, Without<NetEntity>)>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
) {
    // `InteractNpc` is fed in `WorldStage::Net`, ahead of this `Present` system, so a window's
    // open and its hello land on the same frame.
    let active = interact.1;
    let vocal = window_vocal(*prev_active, active);
    if matches!(vocal, WindowVocal::None) {
        return;
    }
    *prev_active = active;

    let (Some(greetings), Some(mut kits), Some(assets)) = (greetings, kits, assets) else {
        return;
    };
    let listener = cam.single().map(|t| t.translation).unwrap_or(Vec3::ZERO);

    // A despawned NPC resolves nothing: the reference's teardown clear is silent. A player target
    // (trade) resolves nothing either, as the NPC sound (`0x720860`) returns for a player.
    let (guid, hello) = match vocal {
        WindowVocal::Hello(g) => (g, true),
        WindowVocal::Goodbye(g) => (g, false),
        WindowVocal::None => return,
    };
    let Some(&npc) = index.0.get(&guid) else {
        return;
    };
    let Some((row, pos)) = resolve(npc, &units, &greetings) else {
        return;
    };
    let npc_fields = units.get(npc).ok().and_then(|(_, _, s)| s).map(|s| &s.0);
    if npc_sound_silenced(self_q.single().ok().map(|s| &s.0), npc_fields) {
        return;
    }
    let kit_id = if hello { row.hello } else { row.goodbye };
    // Both window lines free-pick their take (`variation = -1`).
    play_line(
        npc, kit_id, None, pos, listener, &mut kits, &assets, &mut out, &config,
    );
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
    fn window_vocal_opens_closes_and_suppresses_swap_goodbye() {
        assert_eq!(window_vocal(None, Some(0x1)), WindowVocal::Hello(0x1)); // open
        assert_eq!(window_vocal(Some(0x1), None), WindowVocal::Goodbye(0x1)); // close to nothing
        assert_eq!(window_vocal(Some(0x1), Some(0x2)), WindowVocal::Hello(0x2)); // swap: no goodbye
        assert_eq!(window_vocal(Some(0x1), Some(0x1)), WindowVocal::None); // no change
        assert_eq!(window_vocal(None, None), WindowVocal::None);
    }

    use crate::ui_gossip::GossipState;
    use crate::ui_session::{close_interaction, feed_interact_npc};
    use crate::ui_taxi::TaxiState;
    use crate::ui_trainer::TrainerOpen;
    use benilla_protocol::messages::TaxiMask;

    /// A `HIGHGUID_UNIT` guid, so the sessions treat it as an NPC.
    const NPC: u64 = 0xF130_0000_0000_0042;

    /// The windows a flight master and a trainer open, the interaction close and the interact
    /// target's feed, in the order the plugins run them.
    fn interact_app() -> App {
        let mut app = App::new();
        app.init_resource::<GossipState>()
            .init_resource::<TaxiState>()
            .init_resource::<TrainerOpen>()
            .init_resource::<InteractNpc>()
            .add_systems(Update, feed_interact_npc)
            .add_systems(PostUpdate, close_interaction);
        app
    }

    /// One frame: the vocal the interact target's change produces, as `play_window_greetings`
    /// derives it.
    fn step(app: &mut App, prev: &mut Option<u64>) -> WindowVocal {
        app.update();
        let active = app.world().resource::<InteractNpc>().1;
        let vocal = window_vocal(*prev, active);
        *prev = active;
        vocal
    }

    /// A flight master with no gossip flag (most of Northrend's) opens `SMSG_SHOWTAXINODES` with
    /// no gossip session in front: the taxi map alone sets the interact target (`0x5932e0`), so it
    /// alone plays the hello and, on close, the goodbye.
    #[test]
    fn a_pure_flight_masters_taxi_map_plays_hello_and_goodbye() {
        let mut app = interact_app();
        let mut prev = None;

        app.world_mut()
            .resource_mut::<TaxiState>()
            .open(NPC, 2, TaxiMask(vec![0b10]));
        assert_eq!(step(&mut app, &mut prev), WindowVocal::Hello(NPC));
        assert_eq!(
            step(&mut app, &mut prev),
            WindowVocal::None,
            "no repeat while open"
        );

        app.world_mut().resource_mut::<TaxiState>().clear();
        assert_eq!(step(&mut app, &mut prev), WindowVocal::Goodbye(NPC));
    }

    /// A gossip flight master's "Show me where I can fly" hands off to the taxi map on the same
    /// NPC: the same target (`0x51fce0`), so no second hello and no goodbye; closing the map
    /// closes the gossip behind it and plays the one goodbye.
    #[test]
    fn a_gossip_to_taxi_hand_off_on_one_npc_is_a_silent_swap() {
        let mut app = interact_app();
        let mut prev = None;

        app.world_mut().resource_mut::<GossipState>().npc = Some(NPC);
        assert_eq!(step(&mut app, &mut prev), WindowVocal::Hello(NPC));

        app.world_mut()
            .resource_mut::<TaxiState>()
            .open(NPC, 2, TaxiMask(vec![0b10]));
        assert_eq!(step(&mut app, &mut prev), WindowVocal::None);

        // `CloseTaxiMap()`: the interaction close takes the gossip with it this frame, and the
        // target reads clear on the next.
        app.world_mut().resource_mut::<TaxiState>().clear();
        let closing = step(&mut app, &mut prev);
        let cleared = step(&mut app, &mut prev);
        assert_eq!(app.world().resource::<GossipState>().npc, None);
        assert_eq!(
            [closing, cleared]
                .into_iter()
                .filter(|v| *v != WindowVocal::None)
                .collect::<Vec<_>>(),
            vec![WindowVocal::Goodbye(NPC)],
            "exactly one goodbye",
        );
    }

    /// A gossip→trainer handoff on one NPC is a silent swap, not a close with a goodbye.
    #[test]
    fn gossip_to_trainer_on_one_npc_is_a_silent_swap() {
        let mut app = interact_app();
        let mut prev = None;

        app.world_mut().resource_mut::<GossipState>().npc = Some(NPC);
        assert_eq!(step(&mut app, &mut prev), WindowVocal::Hello(NPC));

        // The gossip closes as the trainer opens on the same NPC.
        app.world_mut().resource_mut::<GossipState>().npc = None;
        app.world_mut().resource_mut::<TrainerOpen>().trainer = Some(NPC);
        assert_eq!(
            step(&mut app, &mut prev),
            WindowVocal::None,
            "gossip→trainer on one NPC is a silent swap — no goodbye",
        );
    }

    /// The NPC sound's gates (`0x720860`): a living player and a living NPC hear it; no player, a
    /// dead or ghost player, an NPC at health 0 or one with `UNIT_DYNFLAG_DEAD` (feign death) do
    /// not.
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
