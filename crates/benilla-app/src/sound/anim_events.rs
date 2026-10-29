//! Routes M2 animation event tags ([`AnimSoundEvent`]) to audio.
//!
//! - `$SND`/`$DSO`: a one-shot kit at the fired key's point.
//! - `$DSL`/`$DSE`: register and release an ambient loop in [`super::emitter_pool`].
//! - `$CSD`: an emote clip's voice; the payload is a literal SoundEntries id, e.g. HumanMale
//!   EmoteLaugh's `$CSD 6923` `HumanMaleEmoteLaugh`.
//! - `$TRD`: the held spell's SpellVisual field-14 strike sound (the mining pick, the smithing
//!   hammer). The held spell is the unit's cast hold, cached client-side from the local GO
//!   interaction (`[CGUnit+0xc8c]`).
//! - `$ESD`: the unit's `UNIT_NPC_EMOTESTATE` → `Emotes.dbc` `EventSoundID`, gated on
//!   `EmoteSpecProc == 2`.
//!
//! `$CST`/`$CSL`/`$CSR` sound nothing here: their handler launches the unit's queued missiles from
//! the event's point, and the only sound is each missile's own flight loop, which the launch in
//! `crate::entities::missile` starts. The `$FD*` fidgets are routed by [`super::creature`], `$AH*`
//! and `$CSS` by [`super::combat`]; any other tag is trace-logged.

use bevy::prelude::*;

use crate::creature_anim::{held_strike_sound, AnimSoundEvent, CastHold, SpellVisuals};
use crate::net::ObjectStore;
use benilla_assets::WorldAssets;
use benilla_world::schedule::WorldStage;

use super::emitter_pool::AmbientEmitterPool;
use super::emote::{play_emote_voice, EmoteSounds, VOICE_ATTACH};
use super::kit::{play_kit, KitRef, SoundCategory, SoundKits};
use super::{AudioListener, SoundConfig, SoundOutput};

/// `$TRD`'s height above the unit's origin (`fadd`); the arm is handed no point.
const TRD_HEIGHT: f32 = 1.0;

pub(super) fn route_anim_events(
    mut events: MessageReader<AnimSoundEvent>,
    // GlobalTransform: `$SND` can fire from a parented visual (a mount's model).
    transforms: Query<&GlobalTransform, Without<Camera3d>>,
    units: Query<(Option<&ObjectStore>, Option<&CastHold>)>,
    // A family-A GameObject's events take the reference's GameObject dispatcher, whose `$DS*` arms
    // differ from the placed-M2 handler the reference's.
    go_lane: Query<(), With<crate::go_anim::GoAnim>>,
    // The attachment read `$CSD` needs.
    attach: crate::entities::AttachPoints,
    kits: Option<ResMut<SoundKits>>,
    assets: Option<Res<WorldAssets>>,
    emotes: Option<Res<EmoteSounds>>,
    spells: Option<Res<crate::ui_action::Spells>>,
    visuals: Option<Res<SpellVisuals>>,
    mut out: NonSendMut<SoundOutput>,
    config: Res<SoundConfig>,
    listener: Res<AudioListener>,
    // Kit ids already warned about, once each: the stream fires at doodad rates, and a few ids
    // (`NightElfLantern01`'s `$DSL(33764)`) are absent from SoundEntries, which the reference
    // plays as silence.
    mut complained: Local<std::collections::HashSet<u32>>,
    mut pool: ResMut<AmbientEmitterPool>,
) {
    if events.is_empty() {
        return;
    }
    let (Some(mut kits), Some(assets)) = (kits, assets) else {
        return;
    };
    let listener = listener.pos;
    let ring = |kits: &mut SoundKits,
                out: &mut SoundOutput,
                kit: u32,
                ev: &AnimSoundEvent,
                complained: &mut std::collections::HashSet<u32>| {
        // The fired key's point where the reference's arm passes it, else the model root.
        let pos = ev
            .pos
            .or_else(|| transforms.get(ev.entity).map(|t| t.translation()).ok());
        if let Err(e) = play_kit(
            kits,
            &assets,
            out,
            &config,
            listener,
            KitRef::Id(kit),
            pos,
            SoundCategory::Sfx,
        ) {
            if complained.insert(kit) {
                warn!("anim event kit {kit}: {e:#} (further reports for this kit suppressed)");
            }
        }
    };
    for ev in events.read() {
        match &ev.ident {
            // `$DSL` starts no sound: it registers the doodad as one emitter of its id in the pool.
            // Re-crossing the marker only repositions the registration, so a wrap never retriggers;
            // a different id releases the old one and registers the new. The pool always loops,
            // whatever the kit's flags; the `0x200` bit is a lane select.
            b"$DSL" if ev.data != 0 => {
                // The emitter sits at the marker, not the model: both lanes pass the event's world
                // point to the pool (via `[ebp+0x10]`, the GameObject arm its `p3`). `$DSL` markers
                // sit up to 67.6 yd off their model's origin.
                if let Some(at) = ev
                    .pos
                    .or_else(|| transforms.get(ev.entity).ok().map(|t| t.translation()))
                {
                    // The placed-M2 arm swaps on a new id; the GameObject arm never compares it and
                    // only repositions a live handle, so Onyxia's lava trap keeps its Stand
                    // `$DSL(8681)` over Custom0's `$DSL(8682)`.
                    if go_lane.contains(ev.entity) {
                        super::emitter_pool::register_keeping_first(
                            &mut pool, ev.entity, ev.data, at, listener,
                        );
                    } else {
                        super::emitter_pool::register(&mut pool, ev.entity, ev.data, at, listener);
                    }
                }
            }
            // `$DSE` releases the doodad's registration, ending a lift's or a machine's loop; the
            // id keeps sounding while another doodad names it. The GameObject dispatcher has no
            // `$DSE` arm: there the registration drops with the state dispatch
            // ([`crate::go_anim::GoStateDispatch`]).
            b"$DSE" if !go_lane.contains(ev.entity) => {
                super::emitter_pool::release(&mut pool, ev.entity);
            }
            // `$SND`/`$DSO` at the fired key: both lanes pass the event's point unchanged to
            // `(id, pos, -1, 1.0f)`.
            b"$SND" | b"$DSO" if ev.data != 0 => {
                ring(&mut kits, &mut out, ev.data, ev, &mut complained);
            }
            // `$CSD` is the emote voice: attachment 17, never the fired key (the CGUnit dispatcher
            // hands it no position, origin + 2.0 z without the attachment), and it stops the unit's
            // previous emote voice.
            b"$CSD" if ev.data != 0 => {
                let root = transforms
                    .get(ev.entity)
                    .map_or(Vec3::ZERO, |t| t.translation());
                let at = attach.point(ev.entity, VOICE_ATTACH, root);
                let store = units.get(ev.entity).ok().and_then(|(store, _)| store);
                if let Err(e) = play_emote_voice(
                    &mut kits, &assets, &mut out, &config, listener, ev.data, ev.entity, store, at,
                ) {
                    if complained.insert(ev.data) {
                        warn!(
                            "anim event kit {}: {e:#} (further reports for this kit suppressed)",
                            ev.data
                        );
                    }
                }
            }
            b"$ESD" => {
                let Some(emotes) = emotes.as_deref() else {
                    continue;
                };
                let state = units
                    .get(ev.entity)
                    .ok()
                    .and_then(|(store, _)| store)
                    .map_or(0, |s| s.0.unit_emote_state());
                if let Some(kit) = (state != 0)
                    .then(|| emotes.state_event_sound(state))
                    .flatten()
                {
                    // At the event's point: the reference pushes it and the reference hands it.
                    ring(&mut kits, &mut out, kit, ev, &mut complained);
                }
            }
            b"$TRD" => {
                let (Some(spells), Some(visuals)) = (spells.as_deref(), visuals.as_deref()) else {
                    continue;
                };
                let hold = units.get(ev.entity).ok().and_then(|(_, h)| h);
                let kit = hold.and_then(|h| held_strike_sound(spells, &visuals.0, h.spell_id));
                if let Some(kit) = kit {
                    // At the unit's origin + 1.0 z: the reference is handed no point and reads the
                    // position itself.
                    let at = transforms
                        .get(ev.entity)
                        .map(|t| t.translation() + Vec3::Y * TRD_HEIGHT)
                        .ok();
                    let unit_root = AnimSoundEvent { pos: at, ..*ev };
                    ring(&mut kits, &mut out, kit, &unit_root, &mut complained);
                }
            }
            other => {
                trace!(
                    "anim event {} (data {}) — no route yet",
                    String::from_utf8_lossy(other),
                    ev.data
                );
            }
        }
    }
}

/// Registration hook for [`super::SoundPlugin`].
pub(super) fn plugin(app: &mut App) {
    app.add_systems(Update, route_anim_events.in_set(WorldStage::Present));
}
