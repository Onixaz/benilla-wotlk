//! The interact target (`0xbd07a8`), the one NPC, player or GameObject every NPC window is bound
//! to: a window's open sets it (`0x51fce0`), whose clear of the held one runs the interaction close
//! (`0x512e60`) and so closes the window bound to it; a window's close clears it (`0x518d50`); and
//! the watchdog (`0x51fde0`), run every frame, clears it when the player leaves its reach or it
//! goes.

use bevy::prelude::*;

use crate::net::{GuidIndex, ObjectStore, SelfPlayer};

/// Owns [`InteractTarget`], its watchdog and its cues.
pub(crate) struct UiSessionPlugin;

impl Plugin for UiSessionPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<InteractTarget>()
            .init_resource::<InteractRange>()
            .add_message::<InteractCue>()
            .add_systems(
                Update,
                (
                    // Ahead of the press, as the world frame's update runs before input; after our
                    // tag, as the frame's creates resolve the active player there (`0x4038f0`).
                    update_interact_target
                        .after(crate::net::apply_net_updates)
                        .after(crate::net::tag_self_player)
                        .before(crate::ui_script::UiPress),
                    deliver_interact_cues.before(benilla_world::schedule::WorldStage::Present),
                ),
            );
    }
}

/// A window bound to the interact target: its NPC, and its client-side close.
pub(crate) trait NpcSession {
    /// The bound NPC's guid; `None` when no window is open.
    fn npc(&self) -> Option<u64>;
    /// The window's client-side close, with no packet: the same clear its close button does.
    fn close(&mut self);
}

/// The reach the watchdog holds the target to, squared (`0xbd0798`), written by the set when the target resolves: `(100/9)²` for a player (`0x51fd71`), `(combat reach + 4)²` for a unit (`0x51fd84`-`0x51fd96`), the type's interact distance squared for a GameObject (`0x51fdc9`); zero or less is never watched (`0x51fe09`).
#[derive(Resource, Default, Debug, PartialEq)]
pub(crate) struct InteractRange(pub(crate) f32);

/// A player target's reach squared, `(100/9)²` (`0xac8a68`).
const PLAYER_INTERACT_SQ: f32 = 123.456_78;

/// A unit target's reach past its combat reach (`0x9e8d2c`).
const UNIT_REACH_PAD: f32 = 4.0;

/// The reach the set stores for `guid`, `None` when it does not resolve (the stored reach stays, `0x51fd58`).
fn interact_range_sq(store: &ObjectStore, guid: u64) -> Option<f32> {
    let f = &store.0;
    if benilla_protocol::guid::is_player(guid) {
        Some(PLAYER_INTERACT_SQ)
    } else if f.is_unit() {
        let reach = f.unit_combat_reach() + UNIT_REACH_PAD;
        Some(reach * reach)
    } else if benilla_protocol::guid::is_gameobject(guid) {
        Some(crate::target::cursor_mode::go_interact_range_sq(
            f.gameobject_type_id(),
        ))
    } else {
        None
    }
}

/// The interact target's watchdog (`0x51fde0`): a held target with a reach is cleared with the goodbye (`0x51ff05`) when it is gone (`0x51fe2e`) or disabled (`0x51fe34`, which benilla's disable meets by taking the guid out of the index at once), when we are (`0x51fe5c`), when a GameObject we cannot interact with (`0x51fe77`: a ghost, or its reaction below neutral) or out of its reach (`0x51fe86`), when a player we cannot cooperate with (`0x51feba`) or a unit we cannot interact with (`0x51fed4`), or when either is past the stored reach (`0x51fef4`); an item is never watched (`0x51fe4f`).
#[allow(clippy::too_many_arguments)] // one Bevy system's full input set
pub(crate) fn update_interact_target(
    target: Res<InteractTarget>,
    range: Res<InteractRange>,
    index: Res<GuidIndex>,
    me: Query<(Entity, &Transform, Option<&ObjectStore>), With<SelfPlayer>>,
    objects: Query<(&Transform, Option<&ObjectStore>)>,
    factions: Option<Res<crate::target::Factions>>,
    reputations: Option<Res<crate::net::Reputations>>,
    party: Option<Res<crate::ui_party::GroupState>>,
    mut queue: Commands,
) {
    let Some(guid) = target.0 else {
        return;
    };
    if range.0 <= 0.0 || benilla_protocol::guid::is_item(guid) {
        return;
    }
    let object = index.0.get(&guid).copied();
    let clear = match (object.and_then(|e| objects.get(e).ok()), me.single().ok()) {
        (None, _) | (_, None) => true,
        (Some((tf, store)), Some((me_entity, me_tf, me_store))) => {
            let dist_sq = tf.translation.distance_squared(me_tf.translation);
            if benilla_protocol::guid::is_gameobject(guid) {
                let type_id = store.map_or(0, |s| s.0.gameobject_type_id());
                let empty = crate::net::Reputations::default();
                let creator = store
                    .and_then(|s| s.0.gameobject_created_by())
                    .and_then(|g| index.0.get(&g))
                    .and_then(|&e| objects.get(e).ok())
                    .and_then(|(_, s)| s);
                let reaction = crate::target::ring::gameobject_reaction(
                    factions.as_deref(),
                    reputations.as_deref().unwrap_or(&empty),
                    crate::target::party_of(party.as_deref()),
                    creator,
                    store.map_or(0, |s| s.0.gameobject_faction()),
                    me_store,
                );
                !crate::target::ring::can_interact_with_gameobject(me_store, reaction)
                    || dist_sq > crate::target::cursor_mode::go_interact_range_sq(type_id)
            } else {
                let refused = if benilla_protocol::guid::is_player(guid) {
                    Some(me_entity) != object
                        && !crate::target::ring::can_cooperate_with_player(
                            factions.as_deref(),
                            store,
                            me_store,
                        )
                } else if store.is_some_and(|s| s.0.is_unit()) {
                    let empty = crate::net::Reputations::default();
                    !crate::target::can_interact(
                        store,
                        factions.as_deref(),
                        reputations.as_deref().unwrap_or(&empty),
                        crate::target::party_of(party.as_deref()),
                        me_store,
                    )
                } else {
                    false
                };
                refused || dist_sq > range.0
            }
        }
    };
    if clear {
        debug!("ui_session: interact target {guid:#x} out of reach or gone — clearing it");
        queue.queue(move |world: &mut World| {
            clear_interact_target(world, guid);
            crate::ui_script::live::fire_handler_events(world);
        });
    }
}

/// What the interact target's set and clear do to the object they name, past the window walk: the NPC sound, the base re-pick and the GameObject's own interaction legs.
#[derive(Message, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InteractCue {
    /// The NPC sound's hello on a set to a new unit (`0x51fda4`) or goodbye on a clear with no new target (`0x518da5`).
    Sound { guid: u64, hello: bool },
    /// The base animation's re-pick (`0x51fdaf`, `0x518db0`).
    RePick(u64),
    /// The GameObject's interaction start (`0x51fdc2` -> `0x70bab0`) or close (`0x5130c4` -> `0x70bac0`).
    GameObject { guid: u64, start: bool },
}

/// The interact target (`0xbd07a8`): every NPC window's open sets it (`0x51fce0`) and the interaction close zeroes it first (`0x512e89`); the `"npc"` token, the portrait, the face-me and the animation read it. Field 1 holds the cues a set or clear made inside a Lua call, played once the call returns.
#[derive(Resource, Default, Debug, PartialEq, Eq)]
pub(crate) struct InteractTarget(pub(crate) Option<u64>, pub(crate) Vec<InteractCue>);

impl InteractTarget {
    /// A target holding `guid`.
    #[cfg(test)]
    pub(crate) fn holding(guid: u64) -> Self {
        Self(Some(guid), Vec::new())
    }

    /// The clear's own legs on a held `guid` (`0x518d50`): a non-player unit plays the goodbye unless a new target follows (`0x518d9b`-`0x518da5`) and re-picks its base (`0x518db0`); then the zero (`0x512e89`) and, past the walk, a GameObject's close (`0x5130c4`). Whether `guid` was held.
    fn clear_held(&mut self, guid: u64, replaced: bool) -> bool {
        if self.0 != Some(guid) {
            return false;
        }
        if benilla_protocol::guid::is_creature_or_pet(guid) {
            if !replaced {
                self.1.push(InteractCue::Sound { guid, hello: false });
            }
            self.1.push(InteractCue::RePick(guid));
        }
        self.0 = None;
        self.close_cue(guid);
        true
    }

    /// The interaction close's GameObject leg (`0x5130b5`-`0x5130c4`).
    fn close_cue(&mut self, guid: u64) {
        if benilla_protocol::guid::is_gameobject(guid) {
            self.1.push(InteractCue::GameObject { guid, start: false });
        }
    }

    /// The clear inside a Lua call (`0x518d50` with 0): the held `guid`'s legs and zero, the window's own leg left to the caller; whether it was held.
    pub(crate) fn clear_in_call(&mut self, guid: u64) -> bool {
        self.clear_held(guid, false)
    }

    /// The interaction close run straight, with no clear ahead of it (the world leave's `0x528dce`): the zero and the GameObject leg; the old target.
    pub(crate) fn close_interaction(&mut self) -> Option<u64> {
        let old = self.0.take()?;
        self.close_cue(old);
        Some(old)
    }
}

/// Hand the cues the target's set and clear made to their players.
fn flush_cues(world: &mut World) {
    let cues = match world.get_resource_mut::<InteractTarget>() {
        Some(mut t) if !t.1.is_empty() => std::mem::take(&mut t.1),
        _ => return,
    };
    if world.contains_resource::<Messages<InteractCue>>() {
        world.write_message_batch(cues);
    }
}

/// The cues a set or clear made inside a Lua call, played after the call.
pub(crate) fn deliver_interact_cues(
    mut target: ResMut<InteractTarget>,
    mut out: MessageWriter<InteractCue>,
) {
    if !target.1.is_empty() {
        out.write_batch(std::mem::take(&mut target.1));
    }
}

/// The interact target's world entity, for the readers that steer or bake by it.
pub(crate) fn interact_entity(target: &InteractTarget, index: &GuidIndex) -> Option<Entity> {
    target.0.and_then(|g| index.0.get(&g).copied())
}

/// The interact target's set (`0x51fce0`): a held one, even the same guid, is cleared first with "a new one follows" (`0x51fd08`-`0x51fd1c`), whose interaction close takes the window bound to it; then the new one is stored (`0x51fd26`), a different non-player unit plays its hello and re-picks its base (`0x51fd82`-`0x51fdaf`), and a GameObject starts its interaction (`0x51fdc2`).
pub(crate) fn set_interact_target(world: &mut World, guid: u64) {
    let held = world.get_resource::<InteractTarget>().and_then(|t| t.0);
    if let Some(old) = held {
        clear_with(world, old, guid != 0);
    }
    let same = held == Some(guid);
    // Stored once the close's events have fired (`0x51fd24`).
    crate::ui_script::live::in_turn(world, move |w| {
        if let Some(mut t) = w.get_resource_mut::<InteractTarget>() {
            t.0 = Some(guid);
            if benilla_protocol::guid::is_creature_or_pet(guid) && !same {
                t.1.push(InteractCue::Sound { guid, hello: true });
                t.1.push(InteractCue::RePick(guid));
            } else if benilla_protocol::guid::is_gameobject(guid) {
                t.1.push(InteractCue::GameObject { guid, start: true });
            }
        }
        flush_cues(w);
        let reach = w
            .get_resource::<GuidIndex>()
            .and_then(|i| i.0.get(&guid).copied())
            .and_then(|e| w.get::<ObjectStore>(e))
            .and_then(|s| interact_range_sq(s, guid));
        if let (Some(reach), Some(mut range)) = (reach, w.get_resource_mut::<InteractRange>()) {
            range.0 = reach;
        }
        if w.contains_non_send::<benilla_ui::script::UiScript>() {
            if let Err(e) = w.run_system_cached_with(crate::ui_unit::push_npc_unit, guid) {
                warn!("ui_session: the \"npc\" unit was not pushed: {e}");
            }
        }
    });
}

/// The interact target's clear (`0x518d50` with 0): a held target equal to `guid` runs the interaction close (`0x518db7` -> `0x512e60`), the goodbye played.
pub(crate) fn clear_interact_target(world: &mut World, guid: u64) {
    clear_with(world, guid, false);
}

/// The clear with its second argument: `replaced` is the setter's "a new target follows" (`0x51fd0c`-`0x51fd16`), which keeps the goodbye quiet.
fn clear_with(world: &mut World, guid: u64, replaced: bool) {
    let cleared = world
        .get_resource_mut::<InteractTarget>()
        .is_some_and(|mut t| t.clear_held(guid, replaced));
    if cleared {
        close_window_on(world, guid);
        flush_cues(world);
    }
}

/// The interact target's clear on whatever it holds (`0x518d50` passed `0xbd07a8` itself): the interaction close always runs.
pub(crate) fn clear_held_interact_target(world: &mut World) {
    if let Some(held) = world.get_resource::<InteractTarget>().and_then(|t| t.0) {
        clear_interact_target(world, held);
    }
}

/// An open window's NPC, read from its resource.
fn npc_of<T: NpcSession + Resource>(world: &World) -> Option<u64> {
    world.get_resource::<T>().and_then(|s| s.npc())
}

/// The interaction close's walk (`0x512e60`) with the target already zeroed: the first window bound to `old`, in its order, closes through its leg.
fn close_window_on(world: &mut World, old: u64) {
    let on = |npc: Option<u64>| npc == Some(old);
    if on(npc_of::<crate::ui_trade::TradeSession>(world)) {
        // `0x512ea4`.
        crate::ui_trade::close_by_interaction(world);
    } else if crate::ui_item_text::close_by_interaction(world, old) {
        // `0x512ecf`.
    } else if on(npc_of::<crate::ui_gossip::GossipState>(world)) {
        // `0x512ef6`.
        crate::ui_gossip::set_npc(world, 0);
    } else if on(world
        .get_non_send_resource::<crate::ui_quest::QuestGiver>()
        .and_then(|q| q.npc()))
    {
        // `0x512f1a`.
        crate::ui_quest::close_interaction(world);
    } else if on(npc_of::<crate::ui_merchant::MerchantOpen>(world)) {
        // `0x512f3a`.
        crate::ui_merchant::close_by_interaction(world);
    } else if on(npc_of::<crate::ui_taxi::TaxiState>(world)) {
        // `0x512f58`.
        crate::ui_taxi::close_by_interaction(world);
    } else if on(npc_of::<crate::ui_trainer::TrainerOpen>(world)) {
        // `0x512f80`.
        crate::ui_trainer::set_none(world);
    } else if on(world
        .get_non_send_resource::<crate::ui_bank::BankOpen>()
        .and_then(|b| b.banker))
    {
        // `0x512fa7`.
        crate::ui_bank::bank_leg(world);
    } else if on(crate::ui_guild_bank::banker(world)) {
        // `0x512fd1`.
        crate::ui_guild_bank::guild_bank_leg(world);
    } else if on(npc_of::<crate::ui_petition::GuildRegistrarState>(world)) {
        // `0x512ff1`.
        crate::ui_petition::close_registrar_by_interaction(world);
    } else if on(npc_of::<crate::ui_tabard::TabardOpen>(world)) {
        // `0x51300e`.
        crate::ui_tabard::close_by_interaction(world);
    } else if on(npc_of::<crate::ui_mail::MailOpen>(world)) {
        // `0x51302d`.
        crate::ui_mail::close_by_interaction(world);
    } else if on(npc_of::<crate::ui_auction::AuctionOpen>(world)) {
        // `0x51304a`.
        crate::ui_auction::close_by_interaction(world);
    } else if on(npc_of::<crate::ui_stable::StableOpen>(world)) {
        // `0x513065`.
        crate::ui_stable::close_by_interaction(world);
    } else if on(npc_of::<crate::ui_petition::PetitionVendorState>(world)) {
        // `0x513083`.
        crate::ui_petition::close_vendor_by_interaction(world);
    } else if on(npc_of::<crate::ui_battlefield::Battlefield>(world)) {
        // `0x51309f`.
        crate::ui_battlefield::close_by_interaction(world);
    }
}

#[cfg(test)]
mod tests;
