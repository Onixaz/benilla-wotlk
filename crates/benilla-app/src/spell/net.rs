//! The spell's packet handlers: the spell book and action bar, the cast lifecycle, cooldowns,
//! channels and spell modifiers. They reach the lifecycle state
//! ([`super::inflight`], [`super::cooldowns`], [`super::mods`]) and the windows' stores through
//! [`Lifecycle`] and [`Scene`].

use std::time::{Duration, Instant};

use benilla_formats::LearnAnnouncement;
use benilla_protocol::messages::{
    ActionButton, SpellCooldown, ACTION_BUTTONS_LOCK, ACTION_BUTTONS_SET,
    ACTION_KIND_EQUIPMENT_SET, ACTION_KIND_SPELL,
};
use bevy::prelude::*;

use super::cast_target::CastTargeting;
use super::learn_place::{self, Learner};
use super::{
    ActiveChannel, AutoRepeatActive, CastCommit, CastLadder, Cooldowns, PendingCast,
    QueuedMeleeSpell,
};
use crate::creature_anim::{CastEvent, CastEventKind, Casting, SpellGoTargets};
use crate::ui_action::{CastErrors, PlayerActions, Spells, UiError, UiErrorKeys};
use crate::ui_cast::{CastBarEdge, CastBarFeed};
use crate::ui_spellbook::LearnedInTab;

use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::ecs::system::SystemParam;

use crate::net::{GuidIndex, NetCommands, NetHandlerApp, ObjectStore, SelfGuid, SelfPlayer};

/// Register the spell's handlers; called from [`super::SpellPlugin`].
pub(super) fn register(app: &mut App) {
    use SessionEventKind as K;
    app.net_handler(K::SpellBook, on_spell_book)
        .net_handler(K::ActionButtons, on_action_buttons)
        .net_handler(K::Disconnected, on_session_end)
        .net_handler(K::SpellLearned, on_spell_learned)
        .net_handler(K::SpellRemoved, on_spell_removed)
        .net_handler(K::SpellSuperceded, on_spell_superceded)
        .net_handler(K::CastResult, on_cast_result)
        .net_handler(K::SpellStart, on_spell_start)
        // Before `on_spell_go`, which clears the in-flight guard the chain tests.
        .net_handler(K::SpellGo, on_own_spell_go)
        .net_handler(K::SpellGo, on_spell_go)
        .net_handler(K::SpellChainTargets, on_spell_chain_targets)
        .net_handler(K::SpellFailed, on_spell_failed)
        .net_handler(K::SpellDelayed, on_spell_delayed)
        .net_handler(K::CancelAutoRepeat, on_cancel_auto_repeat)
        .net_handler(K::SpellCooldowns, on_cooldown_packet)
        .net_handler(K::CooldownEvent, on_cooldown_packet)
        .net_handler(K::ClearCooldown, on_cooldown_packet)
        .net_handler(K::CooldownCheat, on_cooldown_packet)
        .net_handler(K::ModifyCooldown, on_cooldown_packet)
        .net_handler(K::ItemCooldown, on_item_cooldown)
        .net_handler(K::ChannelStart, on_channel)
        .net_handler(K::ChannelUpdate, on_channel)
        .net_handler(K::ResumeCastBar, on_resume_cast_bar)
        .net_handler(K::ResetRangedCombatTimer, on_reset_ranged_combat_timer)
        .net_handler(K::SpellModifier, on_spell_modifier);
}

/// The cast lifecycle's state and catalogs, which every handler here folds its packet into,
/// plus the windows' stores the spell packets write. The cast reply's handler takes the ladder
/// instead ([`on_cast_result`]).
#[derive(SystemParam)]
pub(crate) struct Lifecycle<'w> {
    self_guid: Res<'w, SelfGuid>,
    index: Res<'w, GuidIndex>,
    spells: Option<Res<'w, Spells>>,
    net: Res<'w, NetCommands>,
    actions: ResMut<'w, PlayerActions>,
    learned_in_tab: ResMut<'w, LearnedInTab>,
    summons: Res<'w, super::SummonSlots>,
    companions: ResMut<'w, super::CompanionEdges>,
    multicast: ResMut<'w, super::multicast::MultiCastLists>,
    cast_bar: ResMut<'w, CastBarFeed>,
    pending: ResMut<'w, PendingCast>,
    queued_melee: ResMut<'w, QueuedMeleeSpell>,
    cooldowns: ResMut<'w, Cooldowns>,
    auto_repeat: ResMut<'w, AutoRepeatActive>,
    channel: ResMut<'w, ActiveChannel>,
    spell_mods: ResMut<'w, crate::spell::SpellModifiers>,
    pet_bar: ResMut<'w, crate::ui_pet::PetBar>,
    /// The animation call-order counter: every animation-bearing message stamps `next()`, in packet
    /// order.
    play_seq: ResMut<'w, crate::creature_anim::PlaySeq>,
}

/// The scene a cast lands in: the streamed units, the item store, and the animation, text and
/// loot sinks.
#[derive(SystemParam)]
pub(crate) struct Scene<'w, 's> {
    commands: Commands<'w, 's>,
    casting: Query<'w, 's, &'static Casting>,
    engaged_self: Query<'w, 's, (), (With<crate::creature_anim::Engaged>, With<SelfPlayer>)>,
    stores: Query<'w, 's, &'static mut ObjectStore>,
    items: ResMut<'w, crate::items::Items>,
    loot_latch: ResMut<'w, crate::ui_loot::LootLatch>,
    cast_events: MessageWriter<'w, CastEvent>,
    go_targets: MessageWriter<'w, SpellGoTargets>,
    go_lid: MessageWriter<'w, crate::go_anim::GoLidOpen>,
    sheaths: MessageWriter<'w, crate::creature_anim::SheathRequest>,
    /// Where a GO's extra points' transports are.
    decks: Query<'w, 's, &'static GlobalTransform>,
}

/// `SMSG_INITIAL_SPELLS`, once at login, into the action store the UI feed reads.
fn on_spell_book(In(ev): In<SessionEvent>, mut l: Lifecycle, mut sc: Scene) {
    if let SessionEvent::SpellBook {
        spell_ids,
        cooldowns,
    } = ev
    {
        // The ranged swing time the category adjustment adds, off our own store when streamed.
        let ranged_ms = l
            .self_guid
            .0
            .and_then(|g| l.index.0.get(&g))
            .and_then(|&e| sc.stores.get(e).ok())
            .and_then(|s| s.0.unit_ranged_attack_time())
            .unwrap_or(0);
        spell_book(
            spell_ids,
            cooldowns,
            &mut l.actions,
            &mut l.cooldowns,
            InitialCooldownInputs {
                spells: l.spells.as_deref(),
                items: &mut sc.items,
                net: &l.net,
                mods: &l.spell_mods,
                ranged_ms,
            },
        );
        // The book load files the totems without a refill (`0x6e7f73`).
        l.multicast.lists = Default::default();
        if let Some(spells) = l.spells.as_deref() {
            for d in l
                .actions
                .spells
                .iter()
                .filter_map(|&id| spells.catalog.get(id))
            {
                if d.in_spellbook(&l.summons.0) {
                    l.multicast.learn(d, &spells.catalog);
                }
            }
        }
    }
}

fn on_action_buttons(In(ev): In<SessionEvent>, mut l: Lifecycle) {
    if let SessionEvent::ActionButtons { state, buttons } = ev {
        let player = l.self_guid.0.is_some_and(|g| l.index.0.contains_key(&g));
        action_buttons(state, buttons, player, &mut l.actions, &l.net);
    }
}

/// Every session end returns to glue, whose next world entry zeroes the bar (`0x5a79d0`, via
/// `0x52b550` from `0x405540`) before the login goes out; the lock lives on the player object.
fn on_session_end(In(ev): In<SessionEvent>, mut actions: ResMut<PlayerActions>) {
    if let SessionEvent::Disconnected { .. } = ev {
        clear_bar(&mut actions);
    }
}

/// The bar and its lock emptied without a send; the stored table stays (`0xad9f6c` is only
/// overwritten).
fn clear_bar(actions: &mut PlayerActions) {
    if !actions.buttons.is_empty() {
        actions.buttons.clear();
        actions.dirty = true;
    }
    actions.locked = false;
}

fn on_spell_learned(
    In(ev): In<SessionEvent>,
    mut l: Lifecycle,
    mut errors: ResMut<UiErrorKeys>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
) {
    if let SessionEvent::SpellLearned { spell_id } = ev {
        // `0x4d4db0` on the active player (`0x10`): our own player, once its object has streamed.
        let learner = self_q.iter().next().map(|store| Learner {
            level: store.0.unit_level().unwrap_or(0),
            class: store.0.unit_class().unwrap_or(0),
            form: store.0.unit_shapeshift_form(),
        });
        learned_spell(
            spell_id,
            &mut l.actions,
            l.spells.as_deref(),
            &mut errors,
            &mut l.learned_in_tab,
            learner,
            &l.net,
            &l.summons.0,
        );
        // The totem lists, and a live learn's refill (`0x542d33`, `0x542df1`).
        let display = l
            .spells
            .as_deref()
            .and_then(|s| Some((s, s.catalog.get(spell_id)?)));
        // A live companion learn's line and `COMPANION_LEARNED` wait on its creature (`0x54278e`).
        if learner.is_some()
            && display.is_some_and(|(_, d)| d.in_name_cache() && d.is_companion(&l.summons.0))
        {
            l.companions.learned.push(spell_id);
        }
        if let Some((spells, d)) = display.filter(|(_, d)| d.in_spellbook(&l.summons.0)) {
            let changed = l.multicast.learn(d, &spells.catalog);
            if learner.is_some() && (changed || super::multicast::CALL_SPELLS.contains(&spell_id)) {
                l.multicast.refill_due = true;
            }
        }
    }
}

fn on_spell_removed(In(ev): In<SessionEvent>, mut l: Lifecycle, mut errors: ResMut<UiErrorKeys>) {
    if let SessionEvent::SpellRemoved { spell_id } = ev {
        // The known-spell removal drops a companion from its list (`0x53fad0`).
        let companion = l
            .spells
            .as_deref()
            .and_then(|s| s.catalog.get(spell_id))
            .is_some_and(|d| d.is_companion(&l.summons.0));
        if companion {
            l.companions.unlearned.push(spell_id);
        }
        removed_spell(spell_id, &mut l.actions, l.spells.as_deref(), &mut errors);
        unlearn_totem(&mut l.multicast, spell_id);
    }
}

/// The unlearn's totem sweep (`0x53fe57`); an emptied list or a call spell refills (`0x53ff4f`).
fn unlearn_totem(lists: &mut super::multicast::MultiCastLists, spell_id: u32) {
    if lists.unlearn(spell_id) || super::multicast::CALL_SPELLS.contains(&spell_id) {
        lists.refill_due = true;
    }
}

fn on_spell_superceded(
    In(ev): In<SessionEvent>,
    mut l: Lifecycle,
    mut errors: ResMut<UiErrorKeys>,
) {
    if let SessionEvent::SpellSuperceded {
        old_spell_id,
        new_spell_id,
    } = ev
    {
        superceded_spell(
            old_spell_id,
            new_spell_id,
            &mut l.actions,
            l.spells.as_deref(),
            &mut errors,
            &mut l.learned_in_tab,
        );
        // The pair is unlearn then a live learn (`0x542eb0`).
        unlearn_totem(&mut l.multicast, old_spell_id);
        let display = l
            .spells
            .as_deref()
            .and_then(|s| Some((s, s.catalog.get(new_spell_id)?)));
        if let Some((spells, d)) = display.filter(|(_, d)| d.in_spellbook(&l.summons.0)) {
            if l.multicast.learn(d, &spells.catalog) {
                l.multicast.refill_due = true;
            }
        }
    }
}

/// What the cast-result handler reaches beyond the ladder.
#[derive(SystemParam)]
pub(crate) struct Reply<'w, 's> {
    self_guid: Res<'w, SelfGuid>,
    index: Res<'w, GuidIndex>,
    cast_bar: ResMut<'w, CastBarFeed>,
    casting: Query<'w, 's, &'static Casting>,
    cast_events: MessageWriter<'w, CastEvent>,
    play_seq: ResMut<'w, crate::creature_anim::PlaySeq>,
}

/// The cast-result handler, the one handler that casts: a reply whose spell names a
/// `modalNextSpell` (column 38) chains it through the ladder in the same call (the cast attempt),
/// so it arms the in-flight slot before the next packet.
///
/// The chained cast goes out at the null target guid (`ebx = 0`), so it binds through the target
/// resolve's ordinary walk ([`CastTargeting::context`]) and takes every rung a press takes.
fn on_cast_result(
    In(ev): In<SessionEvent>,
    mut ladder: CastLadder,
    targeting: CastTargeting,
    mut r: Reply,
) {
    if let SessionEvent::CastResult {
        cast_count,
        spell_id,
        success,
        reason,
        arg,
        arg2,
        wire_reason,
    } = ev
    {
        // `SMSG_CAST_FAILED` ends our cast through `0x808200` -> `0x7fecc0` with its reason.
        if let (false, Some(reason), Some(caster)) = (success, reason, r.self_guid.0) {
            r.cast_bar.0.push(CastBarEdge::Failed {
                caster,
                spell_id,
                cast_count,
                reason,
            });
        }
        let chained = cast_result(
            (spell_id, cast_count, wire_reason.unwrap_or(0)),
            success,
            reason,
            [arg, arg2],
            &mut ladder.ecs,
            &r.self_guid,
            &r.index,
            &mut ladder.cast_errors,
            &r.casting,
            &mut r.cast_events,
            &mut ladder.pending,
            &mut ladder.queued_melee,
            &mut ladder.cooldowns,
            &mut ladder.auto_repeat,
            ladder.spells.as_deref(),
            &ladder.commands,
            r.play_seq.next(),
        );
        if let Some(next) = chained {
            let ctx = targeting.context();
            ladder.send(next, &ctx, CastCommit::Spell);
        }
    }
}

/// Our own `SMSG_SPELL_GO` as the cast's success. 12340 sends no success verdict
/// (`SMSG_CAST_FAILED` carries refusals only); its GO handler runs the `modalNextSpell` chain
/// (`0x80e666 call 0x8054f0`) that 1.12.1 ran from the verdict, so the success arm of
/// [`cast_result`] runs here, ahead of [`on_spell_go`].
fn on_own_spell_go(
    In(ev): In<SessionEvent>,
    mut ladder: CastLadder,
    targeting: CastTargeting,
    mut r: Reply,
) {
    let SessionEvent::SpellGo {
        caster, spell_id, ..
    } = ev
    else {
        return;
    };
    if r.self_guid.0 != Some(caster) {
        return;
    }
    let chained = cast_result(
        (spell_id, 0, 0),
        true,
        None,
        [None; 2],
        &mut ladder.ecs,
        &r.self_guid,
        &r.index,
        &mut ladder.cast_errors,
        &r.casting,
        &mut r.cast_events,
        &mut ladder.pending,
        &mut ladder.queued_melee,
        &mut ladder.cooldowns,
        &mut ladder.auto_repeat,
        ladder.spells.as_deref(),
        &ladder.commands,
        r.play_seq.next(),
    );
    if let Some(next) = chained {
        let ctx = targeting.context();
        ladder.send(next, &ctx, CastCommit::Spell);
    }
}

fn on_spell_start(In(ev): In<SessionEvent>, mut l: Lifecycle, mut sc: Scene) {
    if let SessionEvent::SpellStart {
        caster,
        cast_count,
        spell_id,
        cast_flags,
        cast_time_ms,
        target,
        ammo_display_id,
        immunities,
    } = ev
    {
        spell_start(
            caster,
            cast_count,
            spell_id,
            cast_flags,
            cast_time_ms,
            target,
            ammo_display_id,
            immunities,
            &mut sc.commands,
            &l.index,
            &sc.casting,
            &mut sc.cast_events,
            &l.self_guid,
            &mut l.cast_bar,
            &mut l.pending,
            l.spells.as_deref(),
            l.play_seq.next(),
        );
    }
}

/// The loot state our own cast arms the auto-loot flag in.
#[derive(SystemParam)]
struct SpellGoLoot<'w> {
    loot: ResMut<'w, crate::ui_loot::LootState>,
    cfg: Res<'w, crate::ui_loot::LootConfig>,
    script: Option<NonSend<'w, benilla_ui::script::UiScript>>,
}

fn on_spell_go(In(ev): In<SessionEvent>, mut l: Lifecycle, mut sc: Scene, mut arm: SpellGoLoot) {
    if let SessionEvent::SpellGo {
        caster,
        cast_count,
        spell_id,
        cast_flags,
        server_time,
        hits,
        misses,
        target,
        go_target,
        target_mask,
        src,
        dest,
        ammo,
        item_caster,
        trajectory,
        reflects,
        points,
        ..
    } = ev
    {
        // A point on a transport not here is dropped; the rest keep their place now (`0x80e559`).
        let points = points
            .into_iter()
            .filter_map(|(p, deck)| {
                let at = benilla_assets::coords::wow_to_bevy(p);
                if deck == 0 {
                    return Some(crate::entities::DeckPoint {
                        at,
                        deck: None,
                        world: at,
                    });
                }
                let e = *l.index.0.get(&deck)?;
                let world = crate::entities::deck_place(at, sc.decks.get(e).ok()?);
                Some(crate::entities::DeckPoint {
                    at,
                    deck: Some(e),
                    world,
                })
            })
            .collect();
        let deck = |guid: u64| {
            let e = *l.index.0.get(&guid)?;
            Some((e, *sc.decks.get(e).ok()?))
        };
        let wire = dest;
        let src = src.map(|(p, t)| resolve_point(p, t, deck));
        let dest = dest.map(|(p, t)| resolve_point(p, t, deck));
        // A source or dest on a transport not here ends the GO at its release (`0x80e647`).
        let here = !matches!(src, Some(None)) && !matches!(dest, Some(None));
        if benilla_assets::trace::enabled_for("dfx") {
            benilla_assets::trace::line(
                "dfx",
                &format!("go spell={spell_id} wire={wire:?} dest={dest:?} here={here}"),
            );
        }
        let engaged = !sc.engaged_self.is_empty();
        spell_go(
            caster,
            cast_count,
            spell_id,
            cast_flags,
            server_time,
            hits,
            misses,
            reflects,
            target,
            go_target,
            (target_mask, src.flatten(), dest.flatten(), points, here),
            ammo.map(
                |(display_id, inventory_type)| crate::creature_anim::ItemModel {
                    display_id,
                    inventory_type,
                },
            ),
            item_caster,
            trajectory,
            &mut sc.commands,
            &l.index,
            &sc.casting,
            &mut sc.cast_events,
            &mut sc.go_targets,
            &l.self_guid,
            &sc.stores,
            &mut l.cast_bar,
            &mut l.pending,
            &mut l.queued_melee,
            &mut sc.go_lid,
            &mut sc.loot_latch,
            (
                &mut l.cooldowns,
                l.spells.as_deref(),
                &mut sc.items,
                &l.net,
                &mut l.pet_bar,
                &l.spell_mods,
            ),
            (&mut l.auto_repeat, &mut sc.sheaths, engaged),
            l.play_seq.next(),
        );
        // After the chest arm, the effect loop arms the auto-loot flag (`0x80f780`).
        let display = l.spells.as_deref().and_then(|s| s.catalog.get(spell_id));
        if let (true, Some(d)) = (l.self_guid.0 == Some(caster), display) {
            crate::ui_loot::arm_on_spell_go(
                &d.effects,
                &mut arm.loot,
                &arm.cfg,
                arm.script.as_deref(),
                &sc.loot_latch,
                &l.net,
            );
        }
    }
}

fn on_spell_chain_targets(In(ev): In<SessionEvent>, l: Lifecycle, mut sc: Scene) {
    if let SessionEvent::SpellChainTargets {
        caster,
        spell_id,
        targets,
    } = ev
    {
        spell_chain_targets(caster, spell_id, targets, &mut sc.commands, &l.index);
    }
}

fn on_spell_failed(
    In(ev): In<SessionEvent>,
    mut l: Lifecycle,
    mut sc: Scene,
    mut sounds: MessageWriter<crate::net::ServerSoundMessage>,
) {
    if let SessionEvent::SpellFailed {
        other,
        caster,
        cast_count,
        spell_id,
        reason,
        wire_reason,
    } = ev
    {
        if !other {
            spell_failure(
                caster,
                spell_id,
                reason,
                l.spells.as_deref(),
                &l.index,
                &l.self_guid,
                &mut l.auto_repeat,
                &mut sc.commands,
                &l.net,
                &mut sounds,
            );
        }
        // Both handlers end the cast through `0x7fecc0` with the packet's reason (`0x809db3`,
        // `0x806bb0`).
        l.cast_bar.0.push(CastBarEdge::Failed {
            caster,
            spell_id,
            cast_count,
            reason,
        });
        spell_failed_other(
            other,
            caster,
            (spell_id, cast_count, wire_reason),
            &mut sc.commands,
            &l.index,
            &sc.casting,
            &mut sc.cast_events,
            &l.self_guid,
            &mut l.pending,
            &mut l.queued_melee,
            l.play_seq.next(),
        );
    }
}

fn on_spell_delayed(In(ev): In<SessionEvent>, mut l: Lifecycle) {
    if let SessionEvent::SpellDelayed { caster, delay_ms } = ev {
        spell_delayed(
            caster,
            delay_ms,
            &l.self_guid,
            &mut l.cast_bar,
            &mut l.pending,
        );
    }
}

fn on_cancel_auto_repeat(In(ev): In<SessionEvent>, mut l: Lifecycle, mut sc: Scene) {
    if let SessionEvent::CancelAutoRepeat { guid } = ev {
        cancel_auto_repeat(
            guid,
            &mut l.auto_repeat,
            &l.self_guid,
            &l.index,
            &mut sc.commands,
        );
    }
}

/// The five cooldown packets, routed by the caster guid: `SMSG_SPELL_COOLDOWN` and
/// `SMSG_COOLDOWN_CHEAT` through [`addressed_store`], the other three through
/// [`pet_or_player_store`].
fn on_cooldown_packet(In(ev): In<SessionEvent>, mut l: Lifecycle) {
    let Lifecycle {
        self_guid,
        spells,
        cooldowns,
        pet_bar,
        spell_mods,
        ..
    } = &mut l;
    match ev {
        SessionEvent::SpellCooldowns {
            caster,
            flags,
            cooldowns: pairs,
        } => {
            if let Some(store) = addressed_store(caster, self_guid, cooldowns, pet_bar) {
                spell_cooldowns(caster, flags, pairs, spells.as_deref(), spell_mods, store);
            }
        }
        SessionEvent::CooldownEvent { spell_id, caster } => {
            let store = pet_or_player_store(caster, cooldowns, pet_bar);
            cooldown_event(spell_id, caster, store);
        }
        SessionEvent::ClearCooldown { spell_id, caster } => {
            let store = pet_or_player_store(caster, cooldowns, pet_bar);
            clear_cooldown(spell_id, caster, store);
        }
        SessionEvent::CooldownCheat { caster } => {
            if let Some(store) = addressed_store(caster, self_guid, cooldowns, pet_bar) {
                cooldown_cheat(caster, store);
            }
        }
        // `0x804010`'s `0x491` arm skips spell 0 (`0x8040e4`).
        SessionEvent::ModifyCooldown {
            spell_id,
            caster,
            delay_ms,
        } if spell_id != 0 => {
            let store = pet_or_player_store(caster, cooldowns, pet_bar);
            let now = Instant::now();
            // The row, so a category-held cooldown (Elemental Mastery) reads too.
            let row = spells.as_deref().and_then(|s| s.catalog.get(spell_id));
            let left = |store: &Cooldowns| store.info(spell_id, 0, row, now).remaining_ms;
            let before = left(store);
            store.delay(spell_id, delay_ms, now);
            let after = left(store);
            debug!(
                "net: modify cooldown — spell {spell_id} by {delay_ms} ms on {caster:#x}: \
                 {before} → {after} ms left"
            );
        }
        _ => {}
    }
}

fn on_item_cooldown(In(ev): In<SessionEvent>, mut l: Lifecycle, sc: Scene) {
    if let SessionEvent::ItemCooldown {
        item_guid,
        spell_id,
    } = ev
    {
        item_cooldown(item_guid, spell_id, &l.index, &sc.stores, &mut l.cooldowns);
    }
}

fn on_channel(In(ev): In<SessionEvent>, mut l: Lifecycle) {
    match ev {
        SessionEvent::ChannelStart {
            caster,
            spell_id,
            duration_ms,
        } => {
            let own = l.self_guid.0 == Some(caster);
            let streamed = l.index.0.contains_key(&caster);
            channel_start(
                caster,
                own,
                streamed,
                spell_id,
                duration_ms,
                &mut l.channel,
                &mut l.cast_bar,
            );
        }
        SessionEvent::ChannelUpdate {
            caster,
            remaining_ms,
        } => {
            let own = l.self_guid.0 == Some(caster);
            channel_update(caster, own, remaining_ms, &mut l.channel, &mut l.cast_bar);
        }
        _ => {}
    }
}

/// `SMSG_RESUME_CAST_BAR` (`0x8020c0`): a cast or channel already running on a streamed unit,
/// into the cast-bar feed with its back-dated window.
fn on_resume_cast_bar(In(ev): In<SessionEvent>, mut l: Lifecycle) {
    if let SessionEvent::ResumeCastBar(r) = ev {
        debug!(
            "net: resume cast bar — spell {} on {:#x}, {} of {} ms left",
            r.spell_id, r.caster, r.remaining_ms, r.total_ms
        );
        // The caster must be a streamed unit (`0x802150`).
        if l.index.0.contains_key(&r.caster) {
            l.cast_bar.0.push(CastBarEdge::Resume {
                caster: r.caster,
                spell_id: r.spell_id,
                remaining_ms: r.remaining_ms,
                total_ms: r.total_ms,
            });
        }
    }
}

fn on_reset_ranged_combat_timer(In(ev): In<SessionEvent>, mut l: Lifecycle) {
    if let SessionEvent::ResetRangedCombatTimer { time_ms } = ev {
        reset_ranged_combat_timer(
            time_ms,
            &l.actions,
            l.spells.as_deref(),
            &l.summons.0,
            &mut l.cooldowns,
        );
    }
}

/// `SMSG_RESET_RANGED_COMBAT_TIMER` (`0x8071c0`): every book spell with Attributes `0x2`, without
/// AttributesEx2 `0x20000` and AttributesEx4 `0x10000000`, adds a record starting now with
/// `time_ms` as its recovery and no category (`0x80734f`).
fn reset_ranged_combat_timer(
    time_ms: u32,
    actions: &PlayerActions,
    spells: Option<&Spells>,
    summons: &benilla_formats::SummonPropertiesCatalog,
    cooldowns: &mut Cooldowns,
) {
    debug!("net: reset ranged combat timer — {time_ms} ms");
    let Some(spells) = spells else {
        return;
    };
    let now = Instant::now();
    for &id in &actions.spells {
        let Some(d) = spells.catalog.get(id) else {
            continue;
        };
        if d.in_spellbook(summons)
            && d.ranged_speed_cooldown()
            && d.attributes_ex4 & 0x1000_0000 == 0
        {
            cooldowns.start_recovery(id, time_ms, now);
        }
    }
}

/// `SMSG_SPELL_FAILURE`'s own legs (`0x809c70`), on a streamed caster with a `Spell.dbc` row: the
/// school's fizzle sound on the caster (`0x809d45`), and for our own cast failing for ammo
/// (wire `0x34`-`0x36`, NEED_AMMO, NEED_AMMO_POUCH, NEED_EXOTIC_AMMO) the local auto-repeat
/// cancel with its send (`0x809d5b`-`0x809d69`).
fn spell_failure(
    caster: u64,
    spell_id: u32,
    reason: u8,
    spells: Option<&Spells>,
    index: &GuidIndex,
    self_guid: &SelfGuid,
    auto_repeat: &mut AutoRepeatActive,
    commands: &mut Commands,
    net: &crate::net::NetCommands,
    sounds: &mut MessageWriter<crate::net::ServerSoundMessage>,
) {
    let Some(spells) = spells else {
        return;
    };
    let (Some(d), Some(&e)) = (spells.catalog.get(spell_id), index.0.get(&caster)) else {
        return;
    };
    if let Some(sound_id) = spells.catalog.fizzle_sound(d) {
        debug!("net: spell failure {spell_id} on {caster:#x} fizzles sound {sound_id}");
        sounds.write(crate::net::ServerSoundMessage {
            kind: crate::net::ServerSoundKind::ObjectSound,
            sound_id,
            source: Some(e),
        });
    }
    // The shared numbering of wire 0x34-0x36.
    if self_guid.0 == Some(caster) && matches!(reason, 0x2f..=0x31) {
        crate::creature_anim::cancel_auto_repeat_local(Some(e), auto_repeat, commands, net);
    }
}

fn on_spell_modifier(In(ev): In<SessionEvent>, mut l: Lifecycle) {
    if let SessionEvent::SpellModifier {
        flat,
        mask_bit,
        op,
        value,
    } = ev
    {
        set_spell_modifier(flat, mask_bit, op, value, &mut l.spell_mods);
    }
}

/// Which cooldown store `SMSG_SPELL_COOLDOWN` (`0x80704d`) and `SMSG_COOLDOWN_CHEAT` (`0x804110`)
/// address: ours or our pet's (the server sends a pet's cooldowns on the pet's guid). Any other
/// guid is dropped, as the reference drops it.
fn addressed_store<'a>(
    caster: u64,
    self_guid: &SelfGuid,
    player: &'a mut crate::spell::Cooldowns,
    pet: &'a mut crate::ui_pet::PetBar,
) -> Option<&'a mut crate::spell::Cooldowns> {
    if self_guid.0 == Some(caster) {
        Some(player)
    } else if pet.has_bar() && pet.spells.pet_guid == caster {
        Some(&mut pet.cooldowns)
    } else {
        None
    }
}

/// The store `SMSG_COOLDOWN_EVENT`, `SMSG_CLEAR_COOLDOWN` and `SMSG_MODIFY_COOLDOWN` act on
/// (`0x804010`): the pet's for the pet's guid (`0x804033`), the player's for any other
/// (`0x804050`).
fn pet_or_player_store<'a>(
    caster: u64,
    player: &'a mut crate::spell::Cooldowns,
    pet: &'a mut crate::ui_pet::PetBar,
) -> &'a mut crate::spell::Cooldowns {
    if pet.has_bar() && pet.spells.pet_guid == caster {
        &mut pet.cooldowns
    } else {
        player
    }
}

/// What [`Cooldowns::seed_initial`] reads for each `SMSG_INITIAL_SPELLS` cooldown: the spell's
/// row, the item's template, the op-11 tables and our ranged swing time.
struct InitialCooldownInputs<'a> {
    spells: Option<&'a Spells>,
    items: &'a mut crate::items::Items,
    net: &'a crate::net::NetCommands,
    mods: &'a crate::spell::SpellModifiers,
    ranged_ms: u32,
}

/// `SMSG_INITIAL_SPELLS`: the book into the action store, and the active cooldowns, whose wire
/// value is remaining ms, into the cooldown store ([`Cooldowns::seed_initial`]).
fn spell_book(
    spell_ids: Vec<u32>,
    initial_cooldowns: Vec<SpellCooldown>,
    actions: &mut PlayerActions,
    cooldowns: &mut Cooldowns,
    inputs: InitialCooldownInputs,
) {
    debug!(
        "net: spell book — {} spells, {} active cooldown(s)",
        spell_ids.len(),
        initial_cooldowns.len()
    );
    actions.spells = spell_ids.into_iter().collect();
    actions.dirty = true;
    let now = Instant::now();
    for cd in &initial_cooldowns {
        debug!(
            "net: initial cooldown — spell {} item {} category {}: {} / {:#x} ms left",
            cd.spell_id, cd.item_id, cd.category, cd.spell_cd_ms, cd.category_cd_ms
        );
        let spell = inputs.spells.and_then(|s| s.catalog.get(cd.spell_id));
        // The item's slot naming the spell (`0x805b9b`); a template not cached falls back to the
        // spell, as the reference's cache miss does.
        let use_spell = (cd.item_id != 0)
            .then(|| {
                inputs
                    .items
                    .template(u32::from(cd.item_id), 0, inputs.net)
                    .and_then(|t| t.use_spell)
            })
            .flatten();
        let ranged_ms = spell
            .filter(|d| d.ranged_speed_cooldown())
            .map_or(0, |_| inputs.ranged_ms);
        cooldowns.seed_initial(cd, spell, use_spell.as_ref(), inputs.mods, ranged_ms, now);
    }
}

/// `SMSG_ACTION_BUTTONS` (`0x6d8750`). State 2 locks the bar of a streamed player (`0x6d8798`).
/// States 0 and 1 store the table; state 1 also unlocks and puts it on a streamed player's bar
/// (`0x6d87e8`): a slot that [`slot_stands`] is set without a send (`0x6d883c`), every other slot
/// of the 144, an empty one included, is removed with its `CMSG_SET_ACTION_BUTTON` clear
/// (`0x6d882c` -> `0x5aab02`). Otherwise the player's init puts the table on
/// ([`put_action_table_on_init`]).
fn action_buttons(
    state: u8,
    buttons: Vec<ActionButton>,
    player: bool,
    actions: &mut PlayerActions,
    net: &NetCommands,
) {
    debug!(
        "net: action bar — state {state}, {} occupied slots",
        buttons.len()
    );
    if state == ACTION_BUTTONS_LOCK {
        actions.locked |= player;
        return;
    }
    if state > ACTION_BUTTONS_SET {
        return;
    }
    actions.table = Some(buttons.clone());
    if state != ACTION_BUTTONS_SET || !player {
        return;
    }
    actions.locked = false;
    let table: std::collections::HashMap<u8, ActionButton> =
        buttons.into_iter().map(|b| (b.slot, b)).collect();
    for slot in 0..benilla_protocol::messages::ACTION_BUTTON_SLOTS as u8 {
        match table.get(&slot).filter(|b| slot_stands(b, actions, true)) {
            Some(&b) => {
                actions.buttons.insert(slot, b);
                actions.dirty = true;
            }
            None => actions.write_slot(slot, 0, net),
        }
    }
}

/// Whether a table slot goes on the bar: a spell word (kind nibble 0) only if the spell is known,
/// tested on the whole word (`0x6d8805`); with `sets`, an equipment set only if a listed set has
/// its id (`0x6d881f`).
fn slot_stands(b: &ActionButton, actions: &PlayerActions, sets: bool) -> bool {
    let word = b.action | (u32::from(b.kind) << 24);
    if b.kind & 0xF0 == 0 {
        return actions.spells.contains(&word);
    }
    !sets
        || b.kind != ACTION_KIND_EQUIPMENT_SET
        || actions.equipment_sets.iter().any(|s| s.id == b.action)
}

/// The player's init (`0x6e8000`-`0x6e8046`): the stored table's slots go on the bar, a spell slot
/// only if known; nothing is removed.
pub(crate) fn put_action_table_on_init(
    mut cascades: MessageReader<crate::net::WorldEnterCascadeMessage>,
    mut actions: ResMut<PlayerActions>,
) {
    if cascades.read().next().is_none() {
        return;
    }
    let Some(table) = actions.table.clone() else {
        return;
    };
    for b in table {
        if slot_stands(&b, &actions, false) {
            actions.buttons.insert(b.slot, b);
        }
    }
    actions.dirty = true;
}

/// `SMSG_LEARNED_SPELL` (`0x6e7d60`): with our player streamed, `0x6e7b00(spell, slot, 1, 1)`:
/// the rank below leaves the bar for the new one (`0x5aafd0`), then the registrar `0x542030`
/// with its live flag set adds the spell to the book (the spellbook feed diffs it; learning
/// never bars a spell), announces it in chat, flashes its tab, and places a first rank on the
/// first empty button of its page while the character is under level 11
/// ([`super::learn_place`]). Before the player streams, the pair is buffered (`0x6dec70`) and
/// the player's own init replays it as `0x6e7b00(spell, slot, 0, 1)` (`0x6e7f73`): the book
/// alone, no line, no flash, no button.
///
/// Hidden, recipe, companion (#263) and unbooked learns skip tab and bar (`0x54265c`-`0x542856`).
fn learned_spell(
    spell_id: u32,
    actions: &mut PlayerActions,
    spells: Option<&Spells>,
    errors: &mut UiErrorKeys,
    tab_flash: &mut LearnedInTab,
    learner: Option<Learner>,
    net: &NetCommands,
    summons: &benilla_formats::SummonPropertiesCatalog,
) {
    debug!("net: learned spell {spell_id}");
    if actions.spells.insert(spell_id) {
        actions.dirty = true;
    }
    let Some(learner) = learner else {
        debug!("net: learned spell {spell_id} before the player: book only");
        return;
    };
    let packed = spell_id | (u32::from(ACTION_KIND_SPELL) << 24);
    let display = spells.and_then(|s| s.catalog.get(spell_id));
    if let (Some(spells), Some(display)) = (spells, display) {
        for slot in learn_place::lower_rank_slots(actions, &spells.catalog, display) {
            info!("net: bar slot {slot} rank-replaced by learned spell {spell_id}");
            actions.write_slot(slot, packed, net);
        }
    }
    if let Some(d) = display {
        if d.attributes & ATTR_DO_NOT_DISPLAY != 0 {
            return;
        }
        if d.attributes & benilla_formats::SPELL_ATTR_IS_TRADESKILL != 0 {
            announce_learn(spell_id, spells, errors);
            return;
        }
        if d.is_companion(summons) {
            return;
        }
    }
    announce_learn(spell_id, spells, errors);
    if display.is_some_and(|d| d.attributes_ex4 & ATTR_EX4_NOT_IN_SPELLBOOK != 0) {
        return;
    }
    tab_flash.0.push(spell_id);
    if let (Some(spells), Some(display)) = (spells, display) {
        if learn_place::auto_place_allowed(display, learner.level) {
            let page = learn_place::auto_place_page(display, learner, &spells.forms);
            if let Some(slot) = learn_place::first_empty_slot(actions, page) {
                info!("net: learned spell {spell_id} placed on bar slot {slot} (page {page})");
                actions.write_slot(slot, packed, net);
            }
        }
    }
}

/// `Attributes & 0x80`: the registrar's first exit (`0x54265c`).
const ATTR_DO_NOT_DISPLAY: u32 = 0x80;
/// `AttributesEx4 & 0x8000`: kept out of the spellbook, so off the tab and the bar (`0x542856`).
const ATTR_EX4_NOT_IN_SPELLBOOK: u32 = 0x8000;

/// The `ERR_LEARN_*` chat line the registrar prints for a spell learned mid-session; which line,
/// and whether it carries the rank, is [`benilla_formats::SpellDisplay::learn_announcement`]'s. Ids
/// `0x37`/`0x38`/`0x39` are chat type 10 rows, so all three land on the system channel. An unknown
/// id says nothing, the registrar's bail.
fn announce_learn(spell_id: u32, spells: Option<&Spells>, errors: &mut UiErrorKeys) {
    let Some(display) = spells.and_then(|s| s.catalog.get(spell_id)) else {
        return;
    };
    let Some(kind) = display.learn_announcement() else {
        return;
    };
    let (key, arg) = match kind {
        LearnAnnouncement::Spell => ("ERR_LEARN_SPELL_S", display.ranked_name()),
        LearnAnnouncement::Ability => ("ERR_LEARN_ABILITY_S", display.ranked_name()),
        // The recipe arm pushes the bare name: it returns before the rank composer.
        LearnAnnouncement::Recipe => ("ERR_LEARN_RECIPE_S", display.name.clone()),
    };
    errors.0.push(UiError::s(key, arg));
}

/// `SMSG_REMOVED_SPELL`: the spell leaves the book. A talent wipe arrives as these (vmangos
/// `ResetTalents` calls `RemoveSpell` on every rank, each sending `SendSpellRemoved`), and
/// [`crate::ui_talent`] derives rank from the book. The feeds diff `spells`, and
/// [`crate::ui_action::LearnedAbilities`] re-derives from it (the reference's unlearn write site);
/// a bar button naming the spell is left as is.
///
/// It announces "You have unlearned %s." under
/// [`benilla_formats::SpellDisplay::announces_unlearn`]'s gates: the client's spell-removal block,
/// a branch target past its `ret 0x8`, is reached from this packet's arm with the flag set.
fn removed_spell(
    spell_id: u32,
    actions: &mut PlayerActions,
    spells: Option<&Spells>,
    errors: &mut UiErrorKeys,
) {
    debug!("net: removed spell {spell_id}");
    if actions.spells.remove(&spell_id) {
        actions.dirty = true;
    }
    announce_unlearn(spell_id, spells, errors);
}

/// `ERR_SPELL_UNLEARNED_S` with the bare name: the reference pushes `SpellRec+0x1e0` and this path
/// has no rank composer. Which spells say it is
/// [`benilla_formats::SpellDisplay::announces_unlearn`]'s.
fn announce_unlearn(spell_id: u32, spells: Option<&Spells>, errors: &mut UiErrorKeys) {
    let Some(display) = spells.and_then(|s| s.catalog.get(spell_id)) else {
        return;
    };
    if !display.announces_unlearn() {
        return;
    }
    errors
        .0
        .push(UiError::s("ERR_SPELL_UNLEARNED_S", display.name.clone()));
}

/// `SMSG_SUPERCEDED_SPELL`: the new rank replaces the old in the book. The bar follows from the
/// book in `ui_action::ranks`, not from here: vmangos sends no fresh `SMSG_ACTION_BUTTONS`
/// (`Player::learnSpell`), and suppresses this packet while the character loads (`IsInWorld()`).
fn superceded_spell(
    old_spell_id: u32,
    new_spell_id: u32,
    actions: &mut PlayerActions,
    spells: Option<&Spells>,
    errors: &mut UiErrorKeys,
    tab_flash: &mut LearnedInTab,
) {
    debug!("net: superceded spell {old_spell_id} -> {new_spell_id}");
    actions.spells.remove(&old_spell_id);
    actions.spells.insert(new_spell_id);
    actions.dirty = true;
    // A rank-up announces once, like a first learn: the supersede pair calls the unlearn, which
    // prints nothing, then the registrar with `edx = 1`.
    announce_learn(new_spell_id, spells, errors);
    // The same flag flashes the tab of the new rank.
    tab_flash.0.push(new_spell_id);
}

/// The verdict on our cast: a refusal (`SMSG_CAST_FAILED`, benilla's `SMSG_CAST_RESULT`), or
/// with `success` our own `SMSG_SPELL_GO` ([`on_own_spell_go`]).
fn cast_result(
    (spell_id, cast_count, wire_reason): (u32, u8, u8),
    success: bool,
    reason: Option<u8>,
    [arg, arg2]: [Option<u32>; 2],
    commands: &mut Commands,
    self_guid: &SelfGuid,
    index: &GuidIndex,
    cast_errors: &mut CastErrors,
    casting: &Query<&Casting>,
    cast_events: &mut MessageWriter<CastEvent>,
    pending: &mut PendingCast,
    queued_melee: &mut QueuedMeleeSpell,
    cooldowns: &mut Cooldowns,
    auto_repeat: &mut AutoRepeatActive,
    spells: Option<&Spells>,
    net: &crate::net::NetCommands,
    seq: u64,
) -> Option<u32> {
    debug!("net: cast result — spell {spell_id} success={success} reason={reason:?}");
    // Is this the reply to our outstanding cast? Read before either arm clears the guard: both
    // clears are the reference's one.
    let in_flight = pending.committed(Instant::now()) == Some(spell_id);
    if !success {
        // The cast-failed handler clears the GCD armed at send, and the bit-25 revert below drops a
        // parked record; the spell's own recovery starts at SPELL_GO, which a failed cast never
        // reaches. A failing cached auto-repeat spell with reason ≠ 0x17 runs the full local cancel
        // (the `SMSG_CANCEL_AUTO_REPEAT` routine). A deselect arrives as this failure (vmangos
        // `HandleSetSelectionOpcode` → `Spell::cancel` → `SendCastResult(INTERRUPTED)`); target
        // death arrives as [`cancel_auto_repeat`].
        let now = Instant::now();
        // Reason 0x17 DONT_REPORT exits before the GCD clear and the display. The in-flight clear
        // below still runs, as the reference's does once the cast-failed handler returns: vmangos
        // sends 0x17 for real aborts.
        let dont_report = reason == Some(0x17);
        if !dont_report {
            cooldowns.clear_gcd(spell_id, now);
        }
        // The bit-25 revert, past the failure routine and so for DONT_REPORT too: a non-0x3c
        // failure of a cooldown-on-event spell removes its parked record, a failed Feign Death or
        // Stealth (`0x809bd5`-`0x809bf6`).
        if reason != Some(0x3c)
            && spells
                .and_then(|s| s.catalog.get(spell_id))
                .is_some_and(|d| d.cooldown_on_event())
        {
            cooldowns.clear_spell(spell_id);
        }
        let self_e = self_guid.0.and_then(|g| index.0.get(&g)).copied();
        if auto_repeat.0 == Some(spell_id) && reason != Some(0x17) {
            crate::creature_anim::cancel_auto_repeat_local(self_e, auto_repeat, commands, net);
        }
        // Only the failure of the cast the bar shows (keyed to our `Casting`) red-fades it: a
        // pre-start rejection opened no bar, and a proc's failure names another spell.
        let fails_our_cast = self_e.is_some_and(|e| {
            casting
                .get(e)
                .is_ok_and(|c| (c.spell_id, c.cast_count) == (spell_id, cast_count))
        });
        if *crate::net::CAST_TRACE {
            info!("cast-trace: RECV CAST_RESULT failure — spell {spell_id} reason={reason:?}; fails_bar={fails_our_cast}");
        }
        // The in-flight guard opens either way. A queued on-next-swing strike dies here too: the
        // server's melee-slot interrupt (vmangos `Spell::cancel` on the preparing slot) arrives as
        // a failure.
        pending.clear_if(spell_id);
        queued_melee.clear_if(spell_id);
        // The red line is independent of the bar: a pre-start failure still shows it.
        if let Some(reason) = reason {
            if !dont_report {
                cast_errors.0.push(crate::ui_action::CastFail {
                    spell_id,
                    reason,
                    arg,
                    arg2,
                    // `SMSG_CAST_FAILED` is addressed to the caster, and this handler is ours.
                    caster: crate::ui_action::Caster::Player,
                    redisplay: false,
                });
            }
        }
        // Our cast died (vmangos never sends `SMSG_SPELL_FAILURE`): end the self avatar's cast
        // state and precast hold, keyed by spell id.
        if let Some(e) = self_e {
            if fails_our_cast {
                commands.entity(e).remove::<Casting>();
            }
            cast_events.write(CastEvent {
                entity: e,
                spell_id,
                kind: CastEventKind::Fail {
                    interrupt: false,
                    wire_reason,
                },
                cast_count,
                seq,
            });
        }
    }
    // The `modalNextSpell` chain (the cast-result handler): the reply to our in-flight cast
    // finishes its slot, then reads column 38 of its spell; non-zero and not the running repeat,
    // the client casts it at the null target guid. Every hunter shot names 75, Auto Shot.
    // - It fires on success as well as failure: the reference sends a non-failure straight to the
    //   block the failure path reaches.
    // - The slot clears first, or the in-flight rung (our `0x61`) would refuse the chain. vmangos
    //   sends `SMSG_CAST_RESULT` before `SMSG_SPELL_GO` (`Spell.cpp:3681`, `Spell.cpp:3715`), so
    //   the guard is still armed here.
    // - Equal to the running repeat means re-arm, not re-cast, so a second sting never resets the
    //   swing timer; with no record to refresh, we send nothing.
    // The chained spell is returned; `on_cast_result` hands it to the ladder in the same call.
    if in_flight {
        pending.clear_if(spell_id);
        if let Some(next) = spells
            .and_then(|s| s.catalog.get(spell_id))
            .map(|d| d.modal_next_spell)
            .filter(|&next| next != 0 && Some(next) != auto_repeat.0)
        {
            debug!("net: cast result {spell_id} chains modalNextSpell {next}");
            return Some(next);
        }
    }
    None
}

/// `SMSG_SPELL_START`: a unit began a non-triggered cast, instants included (`cast_time_ms == 0`).
fn spell_start(
    caster: u64,
    cast_count: u8,
    spell_id: u32,
    cast_flags: u32,
    cast_time_ms: u32,
    target: Option<u64>,
    ammo_display_id: Option<u32>,
    immunities: Option<(u32, u32)>,
    commands: &mut Commands,
    index: &GuidIndex,
    casting: &Query<&Casting>,
    cast_events: &mut MessageWriter<CastEvent>,
    self_guid: &SelfGuid,
    cast_bar: &mut CastBarFeed,
    pending: &mut PendingCast,
    spells: Option<&crate::ui_action::Spells>,
    seq: u64,
) {
    debug!(
        "net: spell start {spell_id} by {caster:#x} ({cast_time_ms}ms, flags \
         {cast_flags:#x}, target {target:?}, ammo {ammo_display_id:?})"
    );
    // The nocked-ammo refresh (gated on `SpellRec+0x20 & 0x20` or `+0x18 & 0x2`, for any caster): a
    // ranged START attaches the wire's ammo display id, or detaches without one. The model persists
    // through Load/Hold and the fire clip.
    if spells
        .and_then(|s| s.catalog.get(spell_id))
        .is_some_and(|d| d.ranged_attack())
    {
        if let Some(&e) = index.0.get(&caster) {
            match ammo_display_id.filter(|id| *id != 0) {
                Some(display_id) => {
                    commands
                        .entity(e)
                        .insert(crate::creature_anim::NockedAmmo { display_id });
                }
                None => {
                    commands
                        .entity(e)
                        .remove::<crate::creature_anim::NockedAmmo>();
                }
            }
        }
    }
    // A ranged-slot spell (`Attributes & 2`) never opens a bar ([`crate::ui_cast`]'s gate); the
    // trace says so here.
    let ranged_slot = spells
        .and_then(|s| s.catalog.get(spell_id))
        .is_some_and(|d| d.ranged_slot());
    if *crate::net::CAST_TRACE && self_guid.0 == Some(caster) {
        info!(
            "cast-trace: RECV SPELL_START — spell {spell_id} cast_time={cast_time_ms}ms \
             (bar {})",
            if ranged_slot {
                "suppressed: ranged"
            } else {
                "opens"
            }
        );
    }
    // A hidden start (`castFlags & 1`) skips the cast state, precast and UI start (`0x80694e`).
    let hidden = cast_flags & 0x1 != 0;
    // Every caster's start reaches the unit-cast feed, which applies the event gate.
    cast_bar.0.push(CastBarEdge::Start {
        caster,
        spell_id,
        cast_count,
        cast_time_ms,
        immunities,
        hidden,
    });
    if self_guid.0 == Some(caster) && cast_time_ms > 0 {
        // The real cast time replaces the guard's send-time deadline, ranged casts included.
        pending.refine(cast_time_ms, Instant::now());
    }
    if let Some(&e) = index.0.get(&caster).filter(|_| !hidden) {
        let fresh = Casting {
            spell_id,
            cast_count,
            until: (cast_time_ms > 0)
                .then(|| Instant::now() + Duration::from_millis(u64::from(cast_time_ms))),
            listed: true,
        };
        let shows_bar = cast_time_ms > 0
            && spells
                .and_then(|s| s.catalog.get(spell_id))
                .is_none_or(|d| {
                    crate::ui_cast::raises_events(d, self_guid.0 == Some(caster), false)
                });
        match casting.get(e).ok().copied() {
            // Set when none is held (`0x8053cd`).
            None => {
                commands.entity(e).insert(fresh);
            }
            // A timed start with a shown bar replaces another spell's (`0x80544e`).
            Some(held) if shows_bar && held.spell_id != spell_id => {
                commands.entity(e).insert(fresh);
            }
            // A same-spell restart keeps the held count; its new pending cast is listed.
            Some(held) if held.spell_id == spell_id && !held.listed => {
                commands.entity(e).insert(Casting {
                    listed: true,
                    ..held
                });
            }
            Some(_) => {}
        }
        // The precast edge, instants included: their GO follows at once and reaps the hold.
        cast_events.write(CastEvent {
            entity: e,
            spell_id,
            kind: CastEventKind::Start,
            cast_count,
            seq,
        });
    }
}

/// `GAMEOBJECT_TYPE_ID` 3, `CHEST`: the only GameObject type whose `OPEN_LOCK` cast arms the
/// loot-target latch.
const GO_TYPE_CHEST: i32 = 3;

/// A target point's world place and, on a transport, the transport and the point in its frame.
type PlacedPoint = (Vec3, Option<(Entity, Vec3)>);

/// The targets' mask, source and dest points, extra points, and whether every transport the
/// source and dest name is here.
type GoPoints = (
    u32,
    Option<PlacedPoint>,
    Option<PlacedPoint>,
    Vec<crate::entities::DeckPoint>,
    bool,
);

/// A wire point (raw WoW) placed through its transport, the world place at the GO
/// (`0x9ab810`); `None` when the transport is not here (`0x7fd830`).
fn resolve_point(
    raw: [f32; 3],
    transport: u64,
    deck: impl Fn(u64) -> Option<(Entity, GlobalTransform)>,
) -> Option<PlacedPoint> {
    let at = benilla_assets::coords::wow_to_bevy(raw);
    if transport == 0 {
        return Some((at, None));
    }
    let (e, g) = deck(transport)?;
    Some((crate::entities::deck_place(at, &g), Some((e, at))))
}

/// `SMSG_SPELL_GO`: the cast launched. The server schedules damage off `Spell.dbc` Speed; the
/// packet carries the hit and miss lists and the ammo display id, and the flight is rebuilt from
/// the same Speed column ([`SpellGoTargets`]).
fn spell_go(
    caster: u64,
    cast_count: u8,
    spell_id: u32,
    cast_flags: u32,
    server_time: u32,
    hits: Vec<u64>,
    misses: Vec<(u64, u8)>,
    reflects: Vec<(u64, u8)>,
    target: Option<u64>,
    go_target: Option<u64>,
    // The targets' mask with their source, dest and extra points.
    (target_mask, src, dest, points, transports_here): GoPoints,
    ammo: Option<crate::creature_anim::ItemModel>,
    item_caster: Option<u64>,
    trajectory: Option<(f32, u32)>,
    commands: &mut Commands,
    index: &GuidIndex,
    casting: &Query<&Casting>,
    cast_events: &mut MessageWriter<CastEvent>,
    go_targets: &mut MessageWriter<SpellGoTargets>,
    self_guid: &SelfGuid,
    stores: &Query<&mut ObjectStore>,
    cast_bar: &mut CastBarFeed,
    pending: &mut PendingCast,
    queued_melee: &mut QueuedMeleeSpell,
    go_lid: &mut MessageWriter<crate::go_anim::GoLidOpen>,
    // The loot-target latch, armed here for a chest.
    loot_latch: &mut crate::ui_loot::LootLatch,
    // The cooldown store and what it reads; the last member is the pet's, the second bank this
    // handler inserts into ([`pet_go_cooldown`]).
    cooldown_ctx: (
        &mut Cooldowns,
        Option<&Spells>,
        &mut crate::items::Items,
        &crate::net::NetCommands,
        &mut crate::ui_pet::PetBar,
        &crate::spell::SpellModifiers,
    ),
    // The GO-deferred auto-attack start's writes and the attack lock it gates on (`Engaged`, the
    // reference's `[player+0xc48]`).
    attack_ctx: (
        &mut crate::spell::AutoRepeatActive,
        &mut MessageWriter<crate::creature_anim::SheathRequest>,
        bool,
    ),
    seq: u64,
) {
    debug!(
        "net: spell go {spell_id} by {caster:#x} ({} hit(s), {} miss(es), flags \
         {cast_flags:#x}, target {target:?}, go_target {go_target:?}, dest {dest:?}, \
         ammo {ammo:?})",
        hits.len(),
        misses.len()
    );
    // A source or dest on a transport not here ends the GO at the pending cast's release: no
    // feed edge, cooldown, swing, latch or visual (`0x80e647`-`0x80e686`).
    let (cooldowns, spells, items, net_commands, pet_bar, spell_mods) = cooldown_ctx;
    if !transports_here {
        if self_guid.0 == Some(caster) {
            pending.clear_if(spell_id);
            queued_melee.clear_if(spell_id);
        }
        if pet_bar.has_bar() && pet_bar.spells.pet_guid == caster {
            pet_bar.pending_cast = None;
        }
        // The caster's cast state stays; the next start or end clears it (`0x805330`, `0x7fecc0`).
        if let Some(&e) = index.0.get(&caster) {
            if let Ok(&held) = casting.get(e) {
                if (held.spell_id, held.cast_count) == (spell_id, cast_count) {
                    commands.entity(e).insert(Casting {
                        listed: false,
                        ..held
                    });
                }
            }
        }
        return;
    }
    // Every caster's launch reaches the unit-cast feed (`0x80e763`).
    cast_bar.0.push(CastBarEdge::Go {
        caster,
        spell_id,
        cast_count,
        cast_flags,
    });
    // The `use` trace's third link, after `target::click`: a caster we never streamed drops every
    // visual below. A SPELLCASTER GameObject (type 22) is its own caster; vmangos writes that guid
    // as 0 and the decode resolves it (`benilla_protocol::events`' `spell_caster`).
    if benilla_assets::trace::enabled_for("use") {
        benilla_assets::trace::line(
            "use",
            &format!(
                "SPELL_GO spell={spell_id} caster={caster:#x} caster_indexed={} hits={} misses={}",
                index.0.contains_key(&caster),
                hits.len(),
                misses.len()
            ),
        );
    }
    // A cast naming a GameObject goes to the GameObject animation driver, which opens the lid on
    // an open-lock cast, streamed caster or not.
    if let Some(go_guid) = go_target {
        go_lid.write(crate::go_anim::GoLidOpen { go_guid, spell_id });
    }
    let (auto_repeat, sheath, engaged) = attack_ctx;
    let now = Instant::now();
    let display = spells.and_then(|s| s.catalog.get(spell_id));
    // Our own OPEN_LOCK cast on a chest latches it here (the loot open's call, `0x80f6a0`).
    if self_guid.0 == Some(caster) {
        if let Some(go_guid) = go_target {
            let is_open_lock = display.is_some_and(|d| d.open_lock.is_some());
            let is_chest = index
                .0
                .get(&go_guid)
                .and_then(|&e| stores.get(e).ok())
                .is_some_and(|store| store.0.gameobject_type_id() == GO_TYPE_CHEST);
            if is_open_lock && is_chest {
                debug!("net: spell go — chest {go_guid:#x} becomes the loot target (kneel arm)");
                loot_latch.0 = Some(go_guid);
            }
        }
    }
    // Our own launch completes the bar and opens the in-flight guard, both keyed by spell id.
    if self_guid.0 == Some(caster) {
        // Only the cast the bar shows completes it: a proc landing mid-cast (Chilled, 6136) is our
        // own cast too, and must not finish the running bar.
        let completes_our_cast = index.0.get(&caster).is_some_and(|&e| {
            casting
                .get(e)
                .is_ok_and(|c| (c.spell_id, c.cast_count) == (spell_id, cast_count))
        });
        if *crate::net::CAST_TRACE {
            info!("cast-trace: RECV SPELL_GO — spell {spell_id} (self); completes_bar={completes_our_cast}");
        }
        pending.clear_if(spell_id);
        // The queued strike fired on this swing: the queue opens here, like the in-flight finish.
        queued_melee.clear_if(spell_id);

        // The GO-deferred auto-attack start, the complement of [`crate::spell::cast_send`]'s
        // send-time tail: an `AttributesEx2 & 0x100000` spell (the stealth openers and Judgement,
        // 36 rows) starts the swing once the server confirms it. The target is `hits[0]`, else the
        // null pair, which the reference resolves to the selection or else the nearest hostile.
        // Deviation: the fallback is the packet's target guid, never the nearest-hostile acquire,
        // because that would attack something the player never named. Gated on
        // `[player+0xc48] == 0`.
        if display.is_some_and(|d| d.initiates_auto_attack_at_go()) && !engaged {
            if let (Some(&me), Some(guid)) =
                (index.0.get(&caster), hits.first().copied().or(target))
            {
                debug!("net: spell go {spell_id} — deferred auto-attack start at {guid:#x}");
                crate::creature_anim::start_attack_local(
                    me,
                    guid,
                    engaged,
                    false,
                    auto_repeat,
                    sheath,
                    commands,
                    net_commands,
                );
            }
        }

        // Our launch starts the cast's cooldown here, at the GO receive time. The self-insert forks
        // on itemCaster == caster: the spell leg (Category, CategoryRecoveryTime, on-hold from
        // Attributes bit 25) or the item leg (per-slot values with SpellRec fallbacks; a
        // non-resident row pends). `SMSG_SPELL_COOLDOWN` is the server's override; vmangos sends
        // none for a plain cast. A ranged-slot cast folds `UNIT_FIELD_RANGEDATTACKTIME` into the
        // category recovery.
        let ranged_ms = display
            .filter(|d| d.ranged_speed_cooldown())
            .and_then(|_| {
                let e = index.0.get(&caster)?;
                stores.get(*e).ok()?.0.unit_ranged_attack_time()
            })
            .unwrap_or(0);
        // The item's entry, off its own store.
        match item_caster.and_then(|g| stores.get(*index.0.get(&g)?).ok()?.0.object_entry()) {
            Some(entry) => {
                let use_spell = items
                    .template(entry, 0, net_commands)
                    .and_then(|t| t.use_spell)
                    .filter(|u| u.spell_id == spell_id);
                match use_spell {
                    Some(u) => cooldowns.start_item(entry, &u, display, now),
                    // Template not streamed, or naming another spell: the spell-keyed record,
                    // unmodified, as the item leg never applies op 11.
                    None => {
                        if let Some(d) = display {
                            cooldowns.start_spell(spell_id, d, ranged_ms, now, None);
                        }
                    }
                }
            }
            None => {
                if let Some(d) = display {
                    cooldowns.start_spell(spell_id, d, ranged_ms, now, Some(spell_mods));
                }
            }
        }
        if benilla_assets::trace::enabled() {
            if let Some(d) = display {
                let (recovery_ms, category_ms) = spell_mods.spell_cooldowns(d, ranged_ms);
                benilla_assets::trace::line(
                    "cd",
                    &format!(
                        "arm spell={spell_id} rec={}ms cat={}:{}ms (GO self-insert)",
                        recovery_ms, d.category, category_ms
                    ),
                );
            }
        }
    }
    // The pet leg of the same insert: an independent `if`, not an else.
    if pet_bar.has_bar() && pet_bar.spells.pet_guid == caster {
        pet_bar.pending_cast = None;
    }
    if let Some(d) = display.filter(|_| pet_go_cooldown(caster, self_guid, index, stores)) {
        // No ranged pad: the reference is called on the self leg only. Op 11 still applies to both
        // columns off the player's tables, which gate on the spell's family and never the caster.
        pet_bar
            .cooldowns
            .start_spell(spell_id, d, 0, now, Some(spell_mods));
        // The flush that follows fires off the list's generation, which the insert bumped:
        // `ACTIONBAR_UPDATE_COOLDOWN`, `SPELL_UPDATE_COOLDOWN`, `PET_BAR_UPDATE_COOLDOWN`, and no
        // `PET_BAR_UPDATE`.
        if benilla_assets::trace::enabled() {
            let (recovery_ms, category_ms) = spell_mods.spell_cooldowns(d, 0);
            benilla_assets::trace::line(
                "cd",
                &format!(
                    "arm spell={spell_id} rec={}ms cat={}:{}ms (GO pet-insert)",
                    recovery_ms, d.category, category_ms
                ),
            );
        }
    }
    // After the inserts and their flush, the go collects both lists every 120 s
    // (`0x80fe50`-`0x80fe81`).
    if (self_guid.0 == Some(caster) || pet_go_cooldown(caster, self_guid, index, stores))
        && cooldowns.collect_due(now)
    {
        cooldowns.collect(now);
        pet_bar.cooldowns.collect(now);
    }
    // The end clears the held state of this spell and count alone (`0x7fed81`).
    if let Some(&e) = index.0.get(&caster) {
        if casting
            .get(e)
            .is_ok_and(|c| (c.spell_id, c.cast_count) == (spell_id, cast_count))
        {
            commands.entity(e).remove::<Casting>();
        }
        // The GO's one target list: hits then misses, swapped by flag 0x400 (`0x80ec10`,
        // `0x8001ee`), a hit with outcome 0, a reflect with the outcome it sends back.
        let reflect_of = |g: u64| reflects.iter().find(|r| r.0 == g).map_or(0, |r| r.1);
        let hit_codes = hits.iter().map(|&g| (g, 0, 0));
        let miss_codes = misses.iter().map(|&(g, code)| (g, code, reflect_of(g)));
        let list: Vec<(u64, u8, u8)> = if cast_flags & 0x400 != 0 {
            miss_codes.chain(hit_codes).collect()
        } else {
            hit_codes.chain(miss_codes).collect()
        };
        // The hop fill the cast kit consumes, a unit caster's alone (`0x80ed5a`). Deviation:
        // fills on every row, not only a beam's, as only a beam consumes it.
        let object = stores.get(e).is_ok_and(|s| {
            s.0.object_type().is_some_and(|t| {
                !matches!(
                    t,
                    benilla_protocol::messages::ObjectType::Unit
                        | benilla_protocol::messages::ObjectType::Player
                )
            })
        });
        if !object {
            let hops: Vec<u64> = list.iter().map(|&(g, ..)| g).collect();
            fill_chain_hops(caster, e, &hops, commands, index);
        }
        cast_events.write(CastEvent {
            entity: e,
            spell_id,
            kind: CastEventKind::Go,
            cast_count,
            seq,
        });
        // Targets not streamed to us drop out. The miss code picks the victim's dodge or block
        // clip on the missile's arrival.
        let hits: Vec<Entity> = hits
            .iter()
            .filter_map(|g| index.0.get(g).copied())
            .collect();
        let misses: Vec<(Entity, u8)> = misses
            .iter()
            .filter_map(|&(g, code)| index.0.get(&g).map(|&e| (e, code)))
            .collect();
        // A pure dest cast (ground AoE, empty lists) rides the same message: the point is a target.
        let (dest, dest_deck) = dest.map_or((None, None), |(p, d)| (Some(p), d));
        let (src, src_deck) = src.map_or((None, None), |(p, d)| (Some(p), d));
        if !hits.is_empty()
            || !misses.is_empty()
            || dest.is_some()
            || src.is_some()
            || !points.is_empty()
        {
            go_targets.write(SpellGoTargets {
                caster: e,
                spell_id,
                hits,
                misses,
                list,
                target_mask,
                src,
                src_deck,
                dest,
                dest_deck,
                points,
                ammo,
                seq,
                trajectory: trajectory.map(|(pitch, delay_ms)| crate::creature_anim::Trajectory {
                    pitch,
                    // The cast's own aimed speed (`0x733705`), which a GO does not carry.
                    speed: 0.0,
                    delay: delay_ms as f32 / 1000.0,
                    caster,
                    cast_id: cast_count,
                }),
                // A reflect with no outcome of its own lands on the caster (`0x80f44b`).
                reflected: reflects.iter().filter(|(_, r)| *r == 0).count() as u8,
                reflects: reflects
                    .iter()
                    .filter_map(|&(g, r)| Some((*index.0.get(&g)?, r)))
                    .collect(),
                caster_guid: caster,
                server_time,
            });
        }
    }
}

/// `SMSG_SPELL_FAILED_OTHER`: a cast was interrupted; ends the caster's `Casting` as [`spell_go`]
/// does.
fn spell_failed_other(
    other: bool,
    caster: u64,
    (spell_id, cast_count, wire_reason): (u32, u8, u8),
    commands: &mut Commands,
    index: &GuidIndex,
    casting: &Query<&Casting>,
    cast_events: &mut MessageWriter<CastEvent>,
    self_guid: &SelfGuid,
    pending: &mut PendingCast,
    queued_melee: &mut QueuedMeleeSpell,
    seq: u64,
) {
    debug!("net: spell failed (other={other}) {spell_id} by {caster:#x}");
    // Only the cast the bar shows turns it red "Interrupted" (keyed to `Casting`); the guard opens
    // either way.
    if self_guid.0 == Some(caster) {
        let interrupts_our_cast = index.0.get(&caster).is_some_and(|&e| {
            casting
                .get(e)
                .is_ok_and(|c| (c.spell_id, c.cast_count) == (spell_id, cast_count))
        });
        if *crate::net::CAST_TRACE {
            info!("cast-trace: RECV SPELL_FAILED_OTHER — spell {spell_id} (self); interrupts_bar={interrupts_our_cast}");
        }
        pending.clear_if(spell_id);
        // The melee-slot interrupt's other half, beside the failing CAST_RESULT.
        queued_melee.clear_if(spell_id);
    }
    // The end clears the held state of this spell and count alone (`0x7fed81`).
    if let Some(&e) = index.0.get(&caster) {
        let running = casting.get(e).ok();
        if running.is_some_and(|c| (c.spell_id, c.cast_count) == (spell_id, cast_count)) {
            commands.entity(e).remove::<Casting>();
        }
        // `SMSG_SPELL_FAILURE` interrupts the running cast of this spell and count (`0x72ed80`).
        let interrupt =
            !other && running.is_some_and(|c| c.spell_id == spell_id && c.cast_count == cast_count);
        cast_events.write(CastEvent {
            entity: e,
            spell_id,
            kind: CastEventKind::Fail {
                interrupt,
                wire_reason,
            },
            cast_count,
            seq,
        });
    }
}

/// `SMSG_SPELL_DELAYED`: pushback. Any caster's reaches the unit-cast feed, which slides the
/// cast's window (`UNIT_SPELLCAST_DELAYED`); our own also holds the in-flight guard longer.
fn spell_delayed(
    caster: u64,
    delay_ms: u32,
    self_guid: &SelfGuid,
    cast_bar: &mut CastBarFeed,
    pending: &mut PendingCast,
) {
    debug!("net: spell delayed by {caster:#x} (+{delay_ms}ms pushback)");
    cast_bar.0.push(CastBarEdge::Delayed { caster, delay_ms });
    if self_guid.0 == Some(caster) {
        if *crate::net::CAST_TRACE {
            info!("cast-trace: RECV SPELL_DELAYED — +{delay_ms}ms pushback (bar extends, does NOT vanish)");
        }
        // The in-flight guard holds past the stretched end.
        pending.delay(delay_ms, Instant::now());
    }
}

/// `SMSG_CANCEL_AUTO_REPEAT` (`0x72d130`), for the unit its guid names, ignored when it is not
/// streamed. For us the auto-repeat cancel runs without its CMSG (`0x72d17d`): the key drops, which
/// the button's checked state reads (`STOP_AUTOREPEAT_SPELL`), and with it the Load/Hold idle.
/// For any unit, ourselves included, the ranged mode ends. Nothing sheathes.
///
/// AzerothCore sends it on every player autorepeat interrupt, target death included, to everyone
/// in range, so a nearby hunter's cancel names that hunter, not us.
fn cancel_auto_repeat(
    guid: u64,
    auto_repeat: &mut AutoRepeatActive,
    self_guid: &SelfGuid,
    index: &GuidIndex,
    commands: &mut Commands,
) {
    let Some(&e) = index.0.get(&guid) else {
        debug!("net: cancel auto-repeat for {guid:#x}, not streamed — ignored");
        return;
    };
    debug!("net: cancel auto-repeat for {guid:#x}");
    if self_guid.0 == Some(guid) {
        crate::creature_anim::clear_auto_repeat(Some(e), auto_repeat, commands);
    }
    crate::creature_anim::end_ranged_mode(e, commands);
}

/// Whether this `SMSG_SPELL_GO` arms the pet's cooldown bank. The GO handler inserts into two banks
/// (the bank head is the base plus 24 per bank): the self leg bank 0, this leg bank 1, which
/// `SMSG_PET_SPELLS` seeds and `GetPetActionCooldown` reads.
///
/// The gate is the caster's owner off its own descriptor: `charmedBy` when set, else `summonedBy`,
/// compared with the active player's guid. Not the reference's `createdBy` fallback, which would
/// arm it for totems.
///
/// vmangos sends no cooldown packet for a pet's cast: `Creature::AddCooldown`
/// (`Creature.cpp:3259-3282`) sends one only for a charmed non-pet's instant.
fn pet_go_cooldown(
    caster: u64,
    self_guid: &SelfGuid,
    index: &GuidIndex,
    stores: &Query<&mut ObjectStore>,
) -> bool {
    let Some(me) = self_guid.0 else {
        return false;
    };
    index
        .0
        .get(&caster)
        .and_then(|&e| stores.get(e).ok())
        .and_then(|s| s.0.unit_owner(benilla_protocol::OwnerFallback::SummonedBy))
        == Some(me)
}

/// `SMSG_SPELL_COOLDOWN` (`0x806dd0`): server-pushed cooldowns (school lockouts, the pet's list),
/// into the store [`addressed_store`] picked.
fn spell_cooldowns(
    caster: u64,
    flags: u8,
    pairs: Vec<(u32, u32)>,
    spells: Option<&Spells>,
    mods: &crate::spell::SpellModifiers,
    cooldowns: &mut Cooldowns,
) {
    debug!(
        "net: spell cooldowns for {caster:#x} flags {flags:#x} — {} pair(s)",
        pairs.len()
    );
    let now = Instant::now();
    for (spell_id, cooldown_ms) in pairs {
        let display = spells.and_then(|s| s.catalog.get(spell_id));
        cooldowns.apply_wire_cooldown(spell_id, cooldown_ms, flags, display, mods, now);
    }
}

/// `SMSG_ITEM_COOLDOWN`: the fixed 30 s use cooldown, keyed on the item instance's template entry,
/// as the reference resolves it.
fn item_cooldown(
    item_guid: u64,
    spell_id: u32,
    index: &GuidIndex,
    stores: &Query<&mut ObjectStore>,
    cooldowns: &mut Cooldowns,
) {
    debug!("net: item cooldown — item {item_guid:#x} spell {spell_id}");
    let entry = index
        .0
        .get(&item_guid)
        .and_then(|&e| stores.get(e).ok())
        .and_then(|s| s.0.object_entry());
    if let Some(entry) = entry {
        cooldowns.apply_wire_item_cooldown(entry, spell_id, Instant::now());
    }
}

/// `SMSG_COOLDOWN_EVENT` (`(force=0)`): start an on-hold cooldown's parked timers now (Stealth
/// ends, Feign Death drops).
fn cooldown_event(spell_id: u32, caster: u64, cooldowns: &mut Cooldowns) {
    debug!("net: cooldown event — spell {spell_id} on {caster:#x}");
    cooldowns.cooldown_event(spell_id, Instant::now());
}

/// `SMSG_CLEAR_COOLDOWN` (`(force=1)`): remove the spell's record outright.
fn clear_cooldown(spell_id: u32, caster: u64, cooldowns: &mut Cooldowns) {
    debug!("net: clear cooldown — spell {spell_id} on {caster:#x}");
    cooldowns.clear_spell(spell_id);
}

/// `SMSG_COOLDOWN_CHEAT`: the GM reset wipes the self or pet list the guid names.
fn cooldown_cheat(caster: u64, cooldowns: &mut Cooldowns) {
    debug!("net: cooldown cheat (wipe) for {caster:#x}");
    cooldowns.wipe();
}

/// `MSG_CHANNEL_START` (`0x801c90`): a streamed caster's channel reaches the unit-cast feed; our
/// own also arms [`ActiveChannel`]. A duration not above 0 as a signed value (`0x801cc4`), so an
/// infinite one too, or an unstreamed caster (`0x801d5e`), does nothing. The channel animation
/// rides the unit-field pair instead.
fn channel_start(
    caster: u64,
    own: bool,
    streamed: bool,
    spell_id: u32,
    duration_ms: u32,
    channel: &mut ActiveChannel,
    feed: &mut CastBarFeed,
) {
    if duration_ms as i32 <= 0 || !streamed {
        debug!("net: channel start {spell_id} on {caster:#x} for {duration_ms} ms dropped");
        return;
    }
    if own {
        channel.start(spell_id, duration_ms, Instant::now());
    }
    feed.0.push(CastBarEdge::ChannelStart {
        caster,
        spell_id,
        duration_ms,
    });
}

/// `MSG_CHANNEL_UPDATE` (`0x801db0`): a channel's remaining time (`0` is its stop edge).
fn channel_update(
    caster: u64,
    own: bool,
    remaining_ms: u32,
    channel: &mut ActiveChannel,
    feed: &mut CastBarFeed,
) {
    if own {
        channel.update(remaining_ms, Instant::now());
    }
    feed.0.push(CastBarEdge::ChannelUpdate {
        caster,
        remaining_ms,
    });
}

/// `SMSG_SET_FLAT_SPELL_MODIFIER` / `SMSG_SET_PCT_SPELL_MODIFIER`: one store of the absolute value
/// for a `(family bit, op)` pair, never a delta. The out-of-range refusal is
/// [`crate::spell::SpellModifiers::set`]'s.
fn set_spell_modifier(
    flat: bool,
    mask_bit: u8,
    op: u8,
    value: i32,
    mods: &mut crate::spell::SpellModifiers,
) {
    mods.set(flat, mask_bit, op, value);
}

/// The caster's chain-hop array from a wire target list, as the reference: clear, then fill,
/// skipping the unit's own guid; unstreamed targets drop out. Its two callers are the
/// `SMSG_SPELL_UPDATE_CHAIN_TARGETS` handler and the `SMSG_SPELL_GO` handler.
fn fill_chain_hops(
    caster: u64,
    caster_entity: Entity,
    targets: &[u64],
    commands: &mut Commands,
    index: &GuidIndex,
) {
    let hops: Vec<Entity> = targets
        .iter()
        .filter(|&&g| g != caster)
        .filter_map(|g| index.0.get(g).copied())
        .collect();
    commands
        .entity(caster_entity)
        .try_insert(crate::entities::ChainHops(hops));
}

/// `SMSG_SPELL_UPDATE_CHAIN_TARGETS`: the hop list a beam runs through, parked on the caster
/// (`unit+0xd44`: capacity `+0xd44`, count `+0xd48`, data `+0xd4c`, quantum `+0xd50`) and consumed
/// once by the next chain proc. vmangos sends it only for channelled spells
/// (`Spell::SendChannelStart`); other chain spells fill from the GO.
fn spell_chain_targets(
    caster: u64,
    spell_id: u32,
    targets: Vec<u64>,
    commands: &mut Commands,
    index: &GuidIndex,
) {
    debug!(
        "net: chain targets for spell {spell_id} by {caster:#x} — {} hop(s)",
        targets.len()
    );
    if let Some(&e) = index.0.get(&caster) {
        fill_chain_hops(caster, e, &targets, commands, index);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spell::learn_place::Learner;

    /// A point with no transport is its own world place; one on a transport here is placed
    /// through it and keeps its local point; one on a transport not here resolves to nothing
    /// (`0x9ab810`, `0x7fd830`).
    #[test]
    fn a_target_point_is_placed_through_its_transport() {
        use std::f32::consts::FRAC_PI_2;
        const BOAT: u64 = 0x1FC0_0000_0000_0010;
        let deck = Entity::from_raw_u32(7).expect("valid entity id");
        let pose = GlobalTransform::from(
            Transform::from_xyz(100.0, 0.0, 0.0).with_rotation(Quat::from_rotation_y(FRAC_PI_2)),
        );
        let here = |g: u64| (g == BOAT).then_some((deck, pose));
        let raw = [3.0, 0.0, 1.0];
        let local = benilla_assets::coords::wow_to_bevy(raw);
        assert_eq!(resolve_point(raw, 0, here), Some((local, None)));
        let (world, on) = resolve_point(raw, BOAT, here).unwrap();
        assert_eq!(on, Some((deck, local)));
        let want = Vec3::new(100.0, 0.0, 0.0) + Quat::from_rotation_y(FRAC_PI_2) * local;
        assert!(world.distance(want) < 1e-4, "{world} vs {want}");
        assert_eq!(resolve_point(raw, BOAT + 1, here), None);
    }

    /// Another unit's cancel ends only its ranged mode; ours clears our auto-repeat, sending
    /// nothing back.
    #[test]
    fn a_cancel_acts_on_the_unit_its_guid_names() {
        use crate::creature_anim::{AutoRepeatArmed, NockLatch, NockedAmmo};
        use bevy::ecs::system::RunSystemOnce;

        let mut world = World::new();
        let ranged = || (AutoRepeatArmed, NockedAmmo { display_id: 5 }, NockLatch);
        let me = world.spawn(ranged()).id();
        let other = world.spawn(ranged()).id();
        world.insert_resource(GuidIndex([(1, me), (2, other)].into_iter().collect()));
        world.insert_resource(SelfGuid(Some(1)));
        world.insert_resource(AutoRepeatActive(Some(75)));
        let cancel = |world: &mut World, guid: u64| {
            world
                .run_system_once(
                    move |mut auto_repeat: ResMut<AutoRepeatActive>,
                          self_guid: Res<SelfGuid>,
                          index: Res<GuidIndex>,
                          mut commands: Commands| {
                        cancel_auto_repeat(
                            guid,
                            &mut auto_repeat,
                            &self_guid,
                            &index,
                            &mut commands,
                        );
                    },
                )
                .unwrap();
        };

        cancel(&mut world, 2);
        assert_eq!(
            world.resource::<AutoRepeatActive>().0,
            Some(75),
            "ours runs on"
        );
        assert!(world.entity(me).contains::<AutoRepeatArmed>());
        assert!(!world.entity(other).contains::<NockedAmmo>());
        assert!(!world.entity(other).contains::<NockLatch>());
        assert!(!world.entity(other).contains::<AutoRepeatArmed>());

        cancel(&mut world, 3);
        assert_eq!(
            world.resource::<AutoRepeatActive>().0,
            Some(75),
            "unknown guid"
        );

        cancel(&mut world, 1);
        assert_eq!(world.resource::<AutoRepeatActive>().0, None);
        assert!(!world.entity(me).contains::<AutoRepeatArmed>());
        assert!(!world.entity(me).contains::<NockedAmmo>());
    }

    /// A streamed level-1 mage, so the registrar runs its live arms.
    fn live() -> Option<Learner> {
        Some(Learner {
            level: 1,
            class: 8,
            form: 0,
        })
    }

    /// A sender nobody reads: these tests judge the book and the chat line.
    fn no_bar() -> NetCommands {
        NetCommands(crossbeam_channel::unbounded().0)
    }

    /// A catalog holding one row, so the announce tests can name a real `Attributes` value.
    fn catalog_with(id: u32, display: benilla_formats::SpellDisplay) -> Spells {
        let mut spells = Spells::empty_for_tests();
        spells.catalog =
            benilla_formats::SpellCatalog::from_displays([(id, display)].into_iter().collect());
        spells
    }

    /// Heroic Strike carries `SPELL_ATTR_ABILITY`, so the reference lands on `0x38`, the ability
    /// wording, with the argText `"%s (%s)"`.
    #[test]
    fn a_learned_ability_announces_the_ability_line_with_its_rank() {
        let spells = catalog_with(
            78,
            benilla_formats::SpellDisplay {
                name: "Heroic Strike".to_string(),
                rank: Some("Rank 1".to_string()),
                attributes: 0x10,
                ..Default::default()
            },
        );
        let mut actions = PlayerActions::default();
        let mut errors = UiErrorKeys::default();
        let mut flash = LearnedInTab::default();
        learned_spell(
            78,
            &mut actions,
            Some(&spells),
            &mut errors,
            &mut flash,
            live(),
            &no_bar(),
            &Default::default(),
        );

        assert_eq!(errors.0.len(), 1, "one line, once");
        assert_eq!(errors.0[0].key, "ERR_LEARN_ABILITY_S");
        assert_eq!(errors.0[0].arg_s(), Some("Heroic Strike (Rank 1)"));
        // The same live-learn flag gates the tab flash.
        assert_eq!(flash.0, vec![78], "queued for LEARNED_SPELL_IN_TAB");
    }

    /// A plain row is a spell; a tradeskill row is a recipe whose argText drops the rank.
    #[test]
    fn the_spell_and_recipe_arms_pick_their_own_key_and_argument() {
        let spells = catalog_with(
            133,
            benilla_formats::SpellDisplay {
                name: "Fireball".to_string(),
                rank: Some("Rank 1".to_string()),
                attributes: 0x10000,
                ..Default::default()
            },
        );
        let mut errors = UiErrorKeys::default();
        learned_spell(
            133,
            &mut PlayerActions::default(),
            Some(&spells),
            &mut errors,
            &mut LearnedInTab::default(),
            live(),
            &no_bar(),
            &Default::default(),
        );
        assert_eq!(errors.0[0].key, "ERR_LEARN_SPELL_S");
        assert_eq!(errors.0[0].arg_s(), Some("Fireball (Rank 1)"));

        let spells = catalog_with(
            2550,
            benilla_formats::SpellDisplay {
                name: "Cooking".to_string(),
                rank: Some("Apprentice".to_string()),
                attributes: 0x20,
                ..Default::default()
            },
        );
        let mut errors = UiErrorKeys::default();
        learned_spell(
            2550,
            &mut PlayerActions::default(),
            Some(&spells),
            &mut errors,
            &mut LearnedInTab::default(),
            live(),
            &no_bar(),
            &Default::default(),
        );
        assert_eq!(errors.0[0].key, "ERR_LEARN_RECIPE_S");
        assert_eq!(
            errors.0[0].arg_s(),
            Some("Cooking"),
            "the recipe arm pushes the bare name"
        );
    }

    /// The supersede pair reaches the registrar with `edx = 1`: one line, naming the new rank.
    #[test]
    fn a_rank_up_announces_the_new_rank_once() {
        let mut spells = Spells::empty_for_tests();
        spells.catalog = benilla_formats::SpellCatalog::from_displays(
            [
                (
                    78,
                    benilla_formats::SpellDisplay {
                        name: "Heroic Strike".to_string(),
                        rank: Some("Rank 1".to_string()),
                        attributes: 0x10,
                        ..Default::default()
                    },
                ),
                (
                    284,
                    benilla_formats::SpellDisplay {
                        name: "Heroic Strike".to_string(),
                        rank: Some("Rank 2".to_string()),
                        attributes: 0x10,
                        ..Default::default()
                    },
                ),
            ]
            .into_iter()
            .collect(),
        );
        let mut actions = PlayerActions::default();
        actions.spells.insert(78);
        let mut errors = UiErrorKeys::default();
        let mut flash = LearnedInTab::default();
        superceded_spell(
            78,
            284,
            &mut actions,
            Some(&spells),
            &mut errors,
            &mut flash,
        );

        assert_eq!(errors.0.len(), 1, "one line for a rank-up, not two");
        assert_eq!(
            flash.0,
            vec![284],
            "the NEW rank's tab flashes, not the old one's"
        );
        assert_eq!(errors.0[0].key, "ERR_LEARN_ABILITY_S");
        assert_eq!(errors.0[0].arg_s(), Some("Heroic Strike (Rank 2)"));
    }

    /// A `DO_NOT_DISPLAY` row (languages, weapon proficiencies) and an unknown id are silent.
    #[test]
    fn a_do_not_display_spell_and_an_unknown_id_announce_nothing() {
        let spells = catalog_with(
            668,
            benilla_formats::SpellDisplay {
                name: "Language: Common".to_string(),
                attributes: 0xC0,
                ..Default::default()
            },
        );
        let mut errors = UiErrorKeys::default();
        learned_spell(
            668,
            &mut PlayerActions::default(),
            Some(&spells),
            &mut errors,
            &mut LearnedInTab::default(),
            live(),
            &no_bar(),
            &Default::default(),
        );
        learned_spell(
            99999,
            &mut PlayerActions::default(),
            Some(&spells),
            &mut errors,
            &mut LearnedInTab::default(),
            live(),
            &no_bar(),
            &Default::default(),
        );
        assert!(
            errors.0.is_empty(),
            "PASSIVE|DO_NOT_DISPLAY is silent, and so is an id with no record"
        );
    }

    #[test]
    fn learned_spell_adds_to_the_book_once() {
        let mut actions = PlayerActions::default();
        let mut errors = UiErrorKeys::default();
        learned_spell(
            6603,
            &mut actions,
            None,
            &mut errors,
            &mut LearnedInTab::default(),
            live(),
            &no_bar(),
            &Default::default(),
        );
        assert!(actions.spells.contains(&6603));
        assert!(actions.dirty, "a new spell dirties the feed");

        actions.dirty = false;
        learned_spell(
            6603,
            &mut actions,
            None,
            &mut errors,
            &mut LearnedInTab::default(),
            live(),
            &no_bar(),
            &Default::default(),
        );
        assert!(
            !actions.dirty,
            "re-learning a known spell is a no-op (insert returns false)"
        );
    }

    /// What the sender saw, as `(wire slot, packed)`.
    fn sent(rx: &crossbeam_channel::Receiver<crate::net::ClientCommand>) -> Vec<(u8, u32)> {
        rx.try_iter()
            .map(|c| match c {
                crate::net::ClientCommand::SetActionButton { button, packed } => (button, packed),
                other => panic!("a learn sends nothing but SetActionButton: {other:?}"),
            })
            .collect()
    }

    /// The registrar's auto-place (`0x542d44`..`0x542dcf`): a first rank at level 1 lands on the
    /// first empty main-bar button through the `PlaceAction` tail, so one
    /// `CMSG_SET_ACTION_BUTTON` leaves for that slot; a second learn of a second rank replaces it
    /// in place (`0x5aafd0`) and places nothing new.
    #[test]
    fn a_first_rank_is_placed_on_the_first_empty_button_and_sent() {
        let mut spells = Spells::empty_for_tests();
        spells.catalog = benilla_formats::SpellCatalog::from_displays(
            [
                (133, "Rank 1", 0x10000),
                (143, "Rank 2", 0x10000),
                (168, "Rank 1", 0x40), // Frost Armor: passive
            ]
            .into_iter()
            .map(|(id, rank, attributes)| {
                (
                    id,
                    benilla_formats::SpellDisplay {
                        id,
                        name: if id == 168 { "Frost Armor" } else { "Fireball" }.to_string(),
                        rank: Some(rank.to_string()),
                        attributes,
                        ..Default::default()
                    },
                )
            })
            .collect(),
        );
        let (tx, rx) = crossbeam_channel::unbounded();
        let net = NetCommands(tx);
        let mut actions = PlayerActions::default();
        actions.buttons.insert(
            0,
            ActionButton {
                slot: 0,
                action: 6603,
                kind: ACTION_KIND_SPELL,
            },
        );
        let mut errors = UiErrorKeys::default();
        let mut flash = LearnedInTab::default();

        learned_spell(
            133,
            &mut actions,
            Some(&spells),
            &mut errors,
            &mut flash,
            live(),
            &net,
            &Default::default(),
        );
        assert_eq!(actions.buttons[&1].action, 133, "slot 1 is the first empty");
        assert_eq!(
            sent(&rx),
            vec![(1, 133)],
            "the write leaves as a player drop would"
        );
        assert_eq!(errors.0.len(), 1, "announced");
        assert_eq!(flash.0, vec![133]);

        // The passive gate: nothing placed, nothing sent.
        actions.dirty = false;
        learned_spell(
            168,
            &mut actions,
            Some(&spells),
            &mut errors,
            &mut flash,
            live(),
            &net,
            &Default::default(),
        );
        assert!(!actions.buttons.contains_key(&2));
        assert!(sent(&rx).is_empty());

        // Rank 2: the rank-1 button becomes rank 2 (`0x5aafd0` runs before the registrar), and
        // the rank gate (`< 2`) places nothing more.
        learned_spell(
            143,
            &mut actions,
            Some(&spells),
            &mut errors,
            &mut flash,
            live(),
            &net,
            &Default::default(),
        );
        assert_eq!(actions.buttons[&1].action, 143);
        assert!(!actions.buttons.contains_key(&2));
        assert_eq!(sent(&rx), vec![(1, 143)]);

        // At level 11 the level gate closes: the book grows, the bar does not.
        actions.spells.clear();
        actions.buttons.remove(&1);
        learned_spell(
            133,
            &mut actions,
            Some(&spells),
            &mut errors,
            &mut flash,
            Some(Learner {
                level: 11,
                class: 8,
                form: 0,
            }),
            &net,
            &Default::default(),
        );
        assert!(actions.spells.contains(&133));
        assert!(!actions.buttons.contains_key(&1));
        assert!(sent(&rx).is_empty());
    }

    /// Each early exit still joins the book; only the plain spell is flashed and placed.
    #[test]
    fn hidden_recipe_companion_mount_and_unbooked_learns_stay_off_the_tab_and_bar() {
        use benilla_formats::{SpellDisplay, SummonPropertiesCatalog};
        let row = |id: u32, d: SpellDisplay| {
            (
                id,
                SpellDisplay {
                    id,
                    name: format!("Spell {id}"),
                    rank: Some("Rank 1".into()),
                    ..d
                },
            )
        };
        let mut spells = Spells::empty_for_tests();
        spells.catalog = benilla_formats::SpellCatalog::from_displays(
            [
                row(
                    1001,
                    SpellDisplay {
                        attributes: 0x80,
                        ..Default::default()
                    },
                ),
                row(
                    1002,
                    SpellDisplay {
                        attributes: 0x20,
                        ..Default::default()
                    },
                ),
                row(
                    1003,
                    SpellDisplay {
                        effects: [28, 0, 0],
                        effect_misc_value_b: [41, 0, 0],
                        ..Default::default()
                    },
                ),
                row(
                    1004,
                    SpellDisplay {
                        effect_apply_aura: [78, 0, 0],
                        ..Default::default()
                    },
                ),
                row(
                    1005,
                    SpellDisplay {
                        attributes_ex4: 0x8000,
                        ..Default::default()
                    },
                ),
                row(1006, SpellDisplay::default()),
            ]
            .into_iter()
            .collect(),
        );
        let summons = SummonPropertiesCatalog::from_slots([(41, 5)]);
        // (spell, says a line, flashed and placed)
        for (id, line, shown) in [
            (1001, None, false),
            (1002, Some("ERR_LEARN_RECIPE_S"), false),
            (1003, None, false),
            (1004, None, false),
            (1005, Some("ERR_LEARN_SPELL_S"), false),
            (1006, Some("ERR_LEARN_SPELL_S"), true),
        ] {
            let mut actions = PlayerActions::default();
            let mut errors = UiErrorKeys::default();
            let mut flash = LearnedInTab::default();
            learned_spell(
                id,
                &mut actions,
                Some(&spells),
                &mut errors,
                &mut flash,
                live(),
                &no_bar(),
                &summons,
            );
            assert!(actions.spells.contains(&id), "{id} joins the book");
            assert_eq!(errors.0.first().map(|e| e.key), line, "{id}'s line");
            assert_eq!(!flash.0.is_empty(), shown, "{id}'s tab flash");
            assert_eq!(actions.buttons.contains_key(&0), shown, "{id}'s button");
        }
    }

    /// Before our player streams, the pair is buffered and replayed with the live flag clear
    /// (`0x6e7f73`): the book only.
    #[test]
    fn a_learn_before_the_player_is_book_only() {
        let spells = catalog_with(
            133,
            benilla_formats::SpellDisplay {
                name: "Fireball".to_string(),
                rank: Some("Rank 1".to_string()),
                ..Default::default()
            },
        );
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut actions = PlayerActions::default();
        let mut errors = UiErrorKeys::default();
        let mut flash = LearnedInTab::default();
        learned_spell(
            133,
            &mut actions,
            Some(&spells),
            &mut errors,
            &mut flash,
            None,
            &NetCommands(tx),
            &Default::default(),
        );
        assert!(actions.spells.contains(&133));
        assert!(actions.buttons.is_empty(), "no button");
        assert!(sent(&rx).is_empty());
        assert!(errors.0.is_empty(), "no line");
        assert!(flash.0.is_empty(), "no flash");
    }

    /// A rank-up moves the book and marks the store dirty; the bar follows in `ui_action::ranks`.
    #[test]
    fn superceded_spell_swaps_the_book_and_leaves_the_bar_to_the_rank_pass() {
        let mut actions = PlayerActions::default();
        actions.spells.insert(78); // Heroic Strike rank 1, known
        actions.buttons.insert(
            0,
            ActionButton {
                slot: 0,
                action: 78,
                kind: ACTION_KIND_SPELL,
            },
        );

        superceded_spell(
            78,
            284,
            &mut actions,
            None,
            &mut UiErrorKeys::default(),
            &mut LearnedInTab::default(),
        );

        assert!(
            !actions.spells.contains(&78),
            "the old rank leaves the book"
        );
        assert!(
            actions.spells.contains(&284),
            "the new rank enters the book"
        );
        assert_eq!(
            actions.buttons[&0].action, 78,
            "the bar follows from the book, not from here"
        );
        assert!(actions.dirty, "…and `dirty` is what makes it follow");
    }

    /// A proc landing mid-cast (Chilled, 6136) is our own cast, but its GO must not finish the
    /// running Fireball bar: the edge is keyed to our `Casting`.
    #[test]
    fn every_go_reaches_the_unit_cast_feed() {
        use crate::combat_text::CombatTextSpawn;
        use crate::creature_anim::Casting;
        use crate::go_anim::GoLidOpen;
        use crate::net::{Guid, SelfPlayer};
        use bevy::ecs::system::RunSystemOnce;

        let mut app = App::new();
        app.add_message::<CastEvent>()
            .add_message::<SpellGoTargets>()
            .add_message::<CombatTextSpawn>()
            .add_message::<GoLidOpen>()
            .add_message::<crate::creature_anim::SheathRequest>()
            .add_message::<crate::player::StandStateRequest>()
            .init_resource::<GuidIndex>()
            .init_resource::<SelfGuid>()
            .init_resource::<CastBarFeed>()
            .init_resource::<PendingCast>()
            .init_resource::<QueuedMeleeSpell>()
            .init_resource::<Cooldowns>()
            .init_resource::<crate::spell::SpellModifiers>()
            .init_resource::<crate::ui_pet::PetBar>()
            .init_resource::<crate::items::Items>();

        // The self player mid-cast on Fireball (133): the bar is up, `Casting{133}` marks it.
        let self_e = app
            .world_mut()
            .spawn((
                Guid(10),
                SelfPlayer,
                Casting {
                    spell_id: 133,
                    cast_count: 0,
                    until: None,
                    listed: true,
                },
                ObjectStore::default(),
            ))
            .id();
        app.world_mut()
            .resource_mut::<GuidIndex>()
            .0
            .insert(10, self_e);
        app.world_mut().resource_mut::<SelfGuid>().0 = Some(10);

        // One `spell_go` call, parameterized by the completing spell id.
        let fire_go = |app: &mut App, go_spell: u32, here: bool| {
            let (tx, _rx) = crossbeam_channel::unbounded();
            app.world_mut()
                .run_system_once(
                    move |mut commands: Commands,
                          index: Res<GuidIndex>,
                          casting: Query<&Casting>,
                          mut cast_events: MessageWriter<CastEvent>,
                          mut go_targets: MessageWriter<SpellGoTargets>,
                          self_guid: Res<SelfGuid>,
                          stores: Query<&mut ObjectStore>,
                          mut cast_bar: ResMut<CastBarFeed>,
                          mut pending: ResMut<PendingCast>,
                          mut queued_melee: ResMut<QueuedMeleeSpell>,
                          mut go_lid: MessageWriter<GoLidOpen>,
                          mut cooldowns: ResMut<Cooldowns>,
                          mut pet_bar: ResMut<crate::ui_pet::PetBar>,
                          mut items: ResMut<crate::items::Items>,
                          mut sheath: MessageWriter<crate::creature_anim::SheathRequest>| {
                        let net_commands = crate::net::NetCommands(tx.clone());
                        spell_go(
                            10,
                           0,
                            go_spell,
                           0,
                            0,
                            vec![],
                            vec![],
                            Vec::new(),
                            None,
                            None,
                            (0, None, None, Vec::new(), here),
                            None,
                            None,
                            None,
                            &mut commands,
                            &index,
                            &casting,
                            &mut cast_events,
                            &mut go_targets,
                            &self_guid,
                            &stores,
                            &mut cast_bar,
                            &mut pending,
                            &mut queued_melee,
                            &mut go_lid,
                            &mut crate::ui_loot::LootLatch::default(),
                            (
                                &mut cooldowns,
                                None,
                                &mut items,
                                &net_commands,
                                &mut pet_bar,
                                &crate::spell::SpellModifiers::default(),
                            ),
                            (
                                &mut crate::spell::AutoRepeatActive::default(),
                                &mut sheath,
                                false,
                            ),
                            1,
                        );
                    },
                )
                .unwrap();
        };

        // Every GO reaches the feed, a proc's (6136) too; the feed ends only the running cast,
        // keyed by spell and castCount (`0x7fee38`).
        fire_go(&mut app, 6136, true);
        fire_go(&mut app, 133, true);
        let go = |spell_id| CastBarEdge::Go {
            caster: 10,
            spell_id,
            cast_count: 0,
            cast_flags: 0,
        };
        assert_eq!(
            app.world().resource::<CastBarFeed>().0,
            vec![go(6136), go(133)]
        );

        // A dest on a transport not here (`0x80e647`): the pending cast is released, the cast
        // state stays, and no feed edge.
        app.world_mut().entity_mut(self_e).insert(Casting {
            spell_id: 10,
            cast_count: 0,
            until: None,
            listed: true,
        });
        app.world_mut()
            .resource_mut::<PendingCast>()
            .arm(10, Instant::now(), true);
        fire_go(&mut app, 10, false);
        assert_eq!(app.world().resource::<CastBarFeed>().0.len(), 2);
        assert_eq!(
            app.world()
                .resource::<PendingCast>()
                .committed(Instant::now()),
            None
        );
        // Held, but its pending cast is no longer listed (`0x805f30`).
        assert_eq!(
            app.world()
                .get::<Casting>(self_e)
                .map(|c| (c.spell_id, c.listed)),
            Some((10, false))
        );

        // One `spell_start` call: (count, spell, cast time).
        let start = |app: &mut App, count: u8, spell: u32, cast_ms: u32, flags: u32| {
            app.world_mut()
                .run_system_once(
                    move |mut commands: Commands,
                          index: Res<GuidIndex>,
                          casting: Query<&Casting>,
                          mut cast_events: MessageWriter<CastEvent>,
                          self_guid: Res<SelfGuid>,
                          mut cast_bar: ResMut<CastBarFeed>,
                          mut pending: ResMut<PendingCast>| {
                        spell_start(
                            10,
                            count,
                            spell,
                            flags,
                            cast_ms,
                            None,
                            None,
                            None,
                            &mut commands,
                            &index,
                            &casting,
                            &mut cast_events,
                            &self_guid,
                            &mut cast_bar,
                            &mut pending,
                            None,
                            2,
                        );
                    },
                )
                .unwrap();
        };
        let held = |app: &App| {
            app.world()
                .get::<Casting>(self_e)
                .map(|c| (c.spell_id, c.cast_count, c.listed))
        };

        // An instant leaves another spell's held state (`0x8053cd`).
        start(&mut app, 1, 6136, 0, 0);
        assert_eq!(held(&app), Some((10, 0, false)));
        // A timed start of another spell replaces it (`0x80544e`).
        start(&mut app, 1, 133, 2500, 0);
        assert_eq!(held(&app), Some((133, 1, true)));
        // A same-spell restart keeps the held count.
        start(&mut app, 2, 133, 2500, 0);
        assert_eq!(held(&app), Some((133, 1, true)));
        // The end of another count leaves it; its own count clears it (`0x7fed81`).
        fire_go(&mut app, 133, true);
        assert_eq!(held(&app), Some((133, 1, true)));
        app.world_mut().entity_mut(self_e).insert(Casting {
            spell_id: 133,
            cast_count: 0,
            until: None,
            listed: true,
        });
        fire_go(&mut app, 133, true);
        assert_eq!(held(&app), None);
        // With none held, an instant sets it.
        start(&mut app, 3, 6136, 0, 0);
        assert_eq!(held(&app), Some((6136, 3, true)));
        // A hidden start (`castFlags & 1`) leaves it, with no precast edge (`0x80694e`).
        app.world_mut()
            .resource_mut::<Messages<CastEvent>>()
            .clear();
        start(&mut app, 4, 133, 2500, 0x1);
        assert_eq!(held(&app), Some((6136, 3, true)));
        assert!(app.world().resource::<Messages<CastEvent>>().is_empty());
    }

    /// Our channel's start and its server end (`MSG_CHANNEL_UPDATE(0)`) arm and close
    /// [`ActiveChannel`] and reach the unit-cast feed with our guid; another caster's channel
    /// reaches the feed and leaves our mirror alone.
    #[test]
    fn a_channel_arms_the_mirror_and_feeds_its_edges() {
        const BLIZZARD: u32 = 10;
        let t0 = Instant::now();
        let mut feed = CastBarFeed::default();
        let mut channel = ActiveChannel::default();
        channel_start(10, true, true, BLIZZARD, 8_000, &mut channel, &mut feed);
        assert_eq!(
            channel.current(t0 + Duration::from_secs(1)),
            Some(BLIZZARD),
            "the mirror is armed while the channel runs"
        );
        channel_update(10, true, 0, &mut channel, &mut feed);
        assert_eq!(
            channel.current(t0 + Duration::from_secs(1)),
            None,
            "update 0 closes the mirror, so the action button unlights"
        );
        channel_start(0x42, false, true, BLIZZARD, 8_000, &mut channel, &mut feed);
        assert_eq!(channel.current(t0 + Duration::from_secs(1)), None);
        assert_eq!(
            feed.0,
            vec![
                CastBarEdge::ChannelStart {
                    caster: 10,
                    spell_id: BLIZZARD,
                    duration_ms: 8_000
                },
                CastBarEdge::ChannelUpdate {
                    caster: 10,
                    remaining_ms: 0
                },
                CastBarEdge::ChannelStart {
                    caster: 0x42,
                    spell_id: BLIZZARD,
                    duration_ms: 8_000
                },
            ]
        );
    }

    /// `UNIT_FIELD_CHARMEDBY` / `SUMMONEDBY` / `CREATEDBY`, two dwords each.
    const CHARMEDBY: u16 = 10;
    const SUMMONEDBY: u16 = 12;
    const CREATEDBY: u16 = 14;

    /// A unit store whose `field` pair names `owner`.
    fn owned(field: u16, owner: u64) -> ObjectStore {
        ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
            (field, owner as u32),
            (field + 1, (owner >> 32) as u32),
        ]))
    }

    /// One SPELL_GO of `display` (as spell 2649) by `caster` in a world holding us (guid 10) and,
    /// when given, the casting item; `mods` is the player's table. Returns the player bank's and
    /// the pet bank's armed durations, the pet bar's repaint count and the pet list's generation.
    fn go_cooldowns(
        caster: u64,
        store: ObjectStore,
        item: Option<(u64, ObjectStore)>,
        display: fn() -> benilla_formats::SpellDisplay,
        mods: crate::spell::SpellModifiers,
    ) -> (u32, u32, u32, u64) {
        use crate::combat_text::CombatTextSpawn;
        use crate::creature_anim::Casting;
        use crate::go_anim::GoLidOpen;
        use crate::net::{Guid, SelfPlayer};
        use bevy::ecs::system::RunSystemOnce;

        const SPELL: u32 = 2649;
        let spells = crate::ui_action::Spells {
            power_displays: Default::default(),
            difficulties: Default::default(),
            instance_difficulty: Default::default(),
            catalog: benilla_formats::SpellCatalog::from_displays(
                [(SPELL, display())].into_iter().collect(),
            ),
            forms: Default::default(),
            ranges: Default::default(),
            cast_times: Default::default(),
            durations: Default::default(),
            radii: Default::default(),
        };
        let mut app = App::new();
        app.add_message::<CastEvent>()
            .add_message::<SpellGoTargets>()
            .add_message::<CombatTextSpawn>()
            .add_message::<GoLidOpen>()
            .add_message::<crate::creature_anim::SheathRequest>()
            .add_message::<crate::player::StandStateRequest>()
            .init_resource::<GuidIndex>()
            .init_resource::<SelfGuid>()
            .init_resource::<CastBarFeed>()
            .init_resource::<PendingCast>()
            .init_resource::<QueuedMeleeSpell>()
            .init_resource::<Cooldowns>()
            .init_resource::<crate::spell::SpellModifiers>()
            .init_resource::<crate::ui_pet::PetBar>()
            .init_resource::<crate::items::Items>();
        let self_e = app
            .world_mut()
            .spawn((Guid(10), SelfPlayer, ObjectStore::default()))
            .id();
        app.world_mut()
            .resource_mut::<GuidIndex>()
            .0
            .insert(10, self_e);
        if caster != 10 {
            let caster_e = app.world_mut().spawn((Guid(caster), store)).id();
            app.world_mut()
                .resource_mut::<GuidIndex>()
                .0
                .insert(caster, caster_e);
        }
        let item_caster = item.map(|(guid, item_store)| {
            let e = app.world_mut().spawn((Guid(guid), item_store)).id();
            app.world_mut()
                .resource_mut::<GuidIndex>()
                .0
                .insert(guid, e);
            guid
        });
        app.world_mut().resource_mut::<SelfGuid>().0 = Some(10);

        let (tx, _rx) = crossbeam_channel::unbounded();
        app.world_mut()
            .run_system_once(
                move |mut commands: Commands,
                      index: Res<GuidIndex>,
                      casting: Query<&Casting>,
                      mut cast_events: MessageWriter<CastEvent>,
                      mut go_targets: MessageWriter<SpellGoTargets>,
                      self_guid: Res<SelfGuid>,
                      stores: Query<&mut ObjectStore>,
                      mut cast_bar: ResMut<CastBarFeed>,
                      mut pending: ResMut<PendingCast>,
                      mut queued_melee: ResMut<QueuedMeleeSpell>,
                      mut go_lid: MessageWriter<GoLidOpen>,
                      mut cooldowns: ResMut<Cooldowns>,
                      mut pet_bar: ResMut<crate::ui_pet::PetBar>,
                      mut items: ResMut<crate::items::Items>,
                      mut sheath: MessageWriter<crate::creature_anim::SheathRequest>| {
                    let net_commands = crate::net::NetCommands(tx.clone());
                    spell_go(
                        caster,
                        0,
                        SPELL,
                       0,
                        0,
                        vec![],
                        vec![],
                        Vec::new(),
                        None,
                        None,
                        (0, None, None, Vec::new(), true),
                        None,
                        item_caster.or(Some(caster)),
                        None,
                        &mut commands,
                        &index,
                        &casting,
                        &mut cast_events,
                        &mut go_targets,
                        &self_guid,
                        &stores,
                        &mut cast_bar,
                        &mut pending,
                        &mut queued_melee,
                        &mut go_lid,
                        &mut crate::ui_loot::LootLatch::default(),
                        (
                            &mut cooldowns,
                            Some(&spells),
                            &mut items,
                            &net_commands,
                            &mut pet_bar,
                            &mods,
                        ),
                        (
                            &mut crate::spell::AutoRepeatActive::default(),
                            &mut sheath,
                            false,
                        ),
                        1,
                    );
                },
            )
            .unwrap();
        let now = Instant::now();
        let armed = |c: &Cooldowns| c.info(SPELL, 0, Some(&display()), now).duration_ms;
        let world = app.world();
        (
            armed(world.resource::<Cooldowns>()),
            armed(&world.resource::<crate::ui_pet::PetBar>().cooldowns),
            world.resource::<crate::ui_pet::PetBar>().bar_signals,
            world
                .resource::<crate::ui_pet::PetBar>()
                .cooldowns
                .generation,
        )
    }

    /// Growl's shape: a 5 s recovery, no category.
    fn growl() -> benilla_formats::SpellDisplay {
        benilla_formats::SpellDisplay {
            name: "Growl".into(),
            recovery_ms: 5000,
            ..Default::default()
        }
    }

    /// The GO's pet leg arms the pet's bank, never the player's; the owner read falls back to
    /// SUMMONEDBY, not CREATEDBY; a stranger's cast arms neither.
    #[test]
    fn a_pets_own_go_arms_the_pet_bank_and_only_the_pet_bank() {
        let fire = |caster: u64, store: ObjectStore| {
            let (player, pet, signals, generation) = go_cooldowns(
                caster,
                store,
                None,
                growl,
                crate::spell::SpellModifiers::default(),
            );
            (player > 0, pet > 0, signals, generation)
        };

        // Our pet: the PET bank only. The insert moves the list's generation, which is the flush's
        // edge, and signals no `PET_BAR_UPDATE`.
        let (player, pet, signals, generation) = fire(20, owned(SUMMONEDBY, 10));
        assert!(pet, "our pet's GO arms the pet bank");
        assert!(!player, "…and never the player's");
        assert_eq!(generation, 1, "the pet list's flush edge");
        assert_eq!(signals, 0, "the bar's own signal is not the GO's");

        // A charm reads CHARMEDBY first: the same leg, the other field.
        let (_, charmed, _, _) = fire(20, owned(CHARMEDBY, 10));
        assert!(charmed, "a charmed unit's GO arms it too");

        // A totem carries CREATEDBY and no SUMMONEDBY: the reference would accept it, the reference
        // does not.
        let (_, totem, _, _) = fire(30, owned(CREATEDBY, 10));
        assert!(!totem, "CREATEDBY alone is not this leg's owner test");

        // Somebody else's pet: neither bank, no flush.
        let (p2, pet2, _, generation) = fire(40, owned(SUMMONEDBY, 99));
        assert!(!p2 && !pet2, "a stranger's pet arms nothing");
        assert_eq!(generation, 0, "and moves no edge");
    }

    /// Op 11 on the GO insert: the self leg and the pet leg apply the player's table; the item leg
    /// inserts the DBC values, here through its not-streamed fallback.
    #[test]
    fn op_11_shortens_the_self_and_pet_inserts_and_never_the_item_one() {
        fn display() -> benilla_formats::SpellDisplay {
            benilla_formats::SpellDisplay {
                spell_family: 9,
                spell_family_flags: 1,
                ..growl()
            }
        }
        let mods = || {
            let mut m = crate::spell::SpellModifiers::default();
            m.set_class_family(9);
            m.set(true, 0, crate::spell::OP_COOLDOWN, -2000);
            m
        };
        let own = ObjectStore::default();
        assert_eq!(go_cooldowns(10, own, None, display, mods()).0, 3000);
        let pet = go_cooldowns(20, owned(SUMMONEDBY, 10), None, display, mods());
        assert_eq!(pet.1, 3000, "the pet leg takes the player's op 11");
        let item = ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[(3, 6948)]));
        let used = go_cooldowns(
            10,
            ObjectStore::default(),
            Some((50, item)),
            display,
            mods(),
        );
        assert_eq!(used.0, 5000, "the item leg applies no modifier");
    }

    /// Both producers of the chain-hop array, the GO's hit list and the 816 packet, drop the
    /// caster's own guid and clear before they fill.
    #[test]
    fn both_wire_producers_fill_the_casters_chain_hop_array() {
        use crate::combat_text::CombatTextSpawn;
        use crate::creature_anim::Casting;
        use crate::entities::ChainHops;
        use crate::go_anim::GoLidOpen;
        use crate::net::Guid;
        use bevy::ecs::system::RunSystemOnce;

        let mut app = App::new();
        app.add_message::<CastEvent>()
            .add_message::<SpellGoTargets>()
            .add_message::<CombatTextSpawn>()
            .add_message::<GoLidOpen>()
            .add_message::<crate::creature_anim::SheathRequest>()
            .add_message::<crate::player::StandStateRequest>()
            .init_resource::<GuidIndex>()
            .init_resource::<SelfGuid>()
            .init_resource::<CastBarFeed>()
            .init_resource::<PendingCast>()
            .init_resource::<QueuedMeleeSpell>()
            .init_resource::<Cooldowns>()
            .init_resource::<crate::spell::SpellModifiers>()
            .init_resource::<crate::ui_pet::PetBar>()
            .init_resource::<crate::items::Items>();

        // A caster and two streamed victims; guid 40 is never streamed to us.
        let mut spawn = |guid: u64| {
            let e = app
                .world_mut()
                .spawn((Guid(guid), ObjectStore::default()))
                .id();
            app.world_mut()
                .resource_mut::<GuidIndex>()
                .0
                .insert(guid, e);
            e
        };
        let (caster, t1, t2) = (spawn(10), spawn(20), spawn(30));

        let hops = |app: &App| {
            app.world()
                .entity(caster)
                .get::<ChainHops>()
                .map(|h| h.0.clone())
        };

        // The `SMSG_SPELL_GO` handler's fill: the caster itself (vmangos includes self-hits) and an
        // unstreamed target both drop.
        let (tx, _rx) = crossbeam_channel::unbounded();
        app.world_mut()
            .run_system_once(
                move |mut commands: Commands,
                      index: Res<GuidIndex>,
                      casting: Query<&Casting>,
                      mut cast_events: MessageWriter<CastEvent>,
                      mut go_targets: MessageWriter<SpellGoTargets>,
                      self_guid: Res<SelfGuid>,
                      stores: Query<&mut ObjectStore>,
                      mut cast_bar: ResMut<CastBarFeed>,
                      mut pending: ResMut<PendingCast>,
                      mut queued_melee: ResMut<QueuedMeleeSpell>,
                      mut go_lid: MessageWriter<GoLidOpen>,
                      mut cooldowns: ResMut<Cooldowns>,
                      mut pet_bar: ResMut<crate::ui_pet::PetBar>,
                      mut items: ResMut<crate::items::Items>,
                      mut sheath: MessageWriter<crate::creature_anim::SheathRequest>| {
                    let net_commands = crate::net::NetCommands(tx.clone());
                    spell_go(
                        10,
                        0,
                        421, // Chain Lightning
                       0,
                        0,
                        vec![10, 20, 40, 30],
                        vec![],
                        Vec::new(),
                        None,
                        None,
                        (0, None, None, Vec::new(), true),
                        None,
                        None,
                        None,
                        &mut commands,
                        &index,
                        &casting,
                        &mut cast_events,
                        &mut go_targets,
                        &self_guid,
                        &stores,
                        &mut cast_bar,
                        &mut pending,
                        &mut queued_melee,
                        &mut go_lid,
                        &mut crate::ui_loot::LootLatch::default(),
                        (
                            &mut cooldowns,
                            None,
                            &mut items,
                            &net_commands,
                            &mut pet_bar,
                            &crate::spell::SpellModifiers::default(),
                        ),
                        (
                            &mut crate::spell::AutoRepeatActive::default(),
                            &mut sheath,
                            false,
                        ),
                        1,
                    );
                },
            )
            .unwrap();
        assert_eq!(
            hops(&app),
            Some(vec![t1, t2]),
            "the GO fills the array in wire order, minus the caster and the unstreamed target"
        );

        // The 816 packet, vmangos's for a channelled chain, clears the GO's list first.
        app.world_mut()
            .run_system_once(move |mut commands: Commands, index: Res<GuidIndex>| {
                spell_chain_targets(10, 689, vec![30, 10], &mut commands, &index);
            })
            .unwrap();
        assert_eq!(
            hops(&app),
            Some(vec![t2]),
            "clear-before-fill, caster dropped"
        );
    }

    /// A deselect during Auto Shot arrives as a `SMSG_CAST_RESULT` failure of the cached repeat
    /// (vmangos `HandleSetSelectionOpcode` → `Spell::cancel`), and the reference's jump into the
    /// reference makes it the full cancel: the key and the shooting idle both drop.
    #[test]
    fn a_cast_result_fail_of_the_cached_auto_repeat_spell_disarms_the_shooting_idle() {
        use crate::creature_anim::AutoRepeatArmed;
        use crate::net::{Guid, SelfPlayer};
        use bevy::ecs::system::RunSystemOnce;

        let mut app = App::new();
        app.add_message::<CastEvent>()
            .init_resource::<GuidIndex>()
            .init_resource::<SelfGuid>()
            .init_resource::<CastErrors>()
            .init_resource::<crate::ui_action::UiErrorKeys>()
            .init_resource::<CastBarFeed>()
            .init_resource::<PendingCast>()
            .init_resource::<QueuedMeleeSpell>()
            .init_resource::<Cooldowns>()
            .init_resource::<crate::spell::SpellModifiers>()
            .init_resource::<AutoRepeatActive>();

        let self_e = app
            .world_mut()
            .spawn((
                Guid(10),
                SelfPlayer,
                AutoRepeatArmed,
                crate::creature_anim::NockedAmmo { display_id: 5996 },
            ))
            .id();
        app.world_mut()
            .resource_mut::<GuidIndex>()
            .0
            .insert(10, self_e);
        app.world_mut().resource_mut::<SelfGuid>().0 = Some(10);
        app.world_mut().resource_mut::<AutoRepeatActive>().0 = Some(75);

        let (tx, rx) = crossbeam_channel::unbounded();
        let fire_fail = |app: &mut App, reason: u8| {
            let tx = tx.clone();
            app.world_mut()
                .run_system_once(
                    move |mut commands: Commands,
                          self_guid: Res<SelfGuid>,
                          index: Res<GuidIndex>,
                          mut cast_errors: ResMut<CastErrors>,
                          casting: Query<&Casting>,
                          mut cast_events: MessageWriter<CastEvent>,
                          mut pending: ResMut<PendingCast>,
                          mut queued_melee: ResMut<QueuedMeleeSpell>,
                          mut cooldowns: ResMut<Cooldowns>,
                          mut auto_repeat: ResMut<AutoRepeatActive>| {
                        let net = crate::net::NetCommands(tx.clone());
                        cast_result(
                            (75, 0, 0),
                            false,
                            Some(reason),
                            [None; 2],
                            &mut commands,
                            &self_guid,
                            &index,
                            &mut cast_errors,
                            &casting,
                            &mut cast_events,
                            &mut pending,
                            &mut queued_melee,
                            &mut cooldowns,
                            &mut auto_repeat,
                            None,
                            &net,
                            1,
                        );
                    },
                )
                .unwrap();
        };

        // Reason 0x17 is the one skip: armed stays armed.
        fire_fail(&mut app, 0x17);
        assert_eq!(app.world().resource::<AutoRepeatActive>().0, Some(75));
        assert!(app
            .world()
            .entity(self_e)
            .get::<AutoRepeatArmed>()
            .is_some());

        // SPELL_FAILED_INTERRUPTED, the deselect's wire form, runs the full cancel.
        fire_fail(&mut app, 0x1e);
        assert_eq!(
            app.world().resource::<AutoRepeatActive>().0,
            None,
            "the autorepeat key drops"
        );
        assert!(
            app.world()
                .entity(self_e)
                .get::<AutoRepeatArmed>()
                .is_none(),
            "the shooting-idle gate drops with it — the stuck Load/Hold stance"
        );
        assert!(
            app.world()
                .entity(self_e)
                .get::<crate::creature_anim::NockedAmmo>()
                .is_none(),
            "the cancel un-nocks"
        );
        let sent: Vec<_> = rx.try_iter().collect();
        assert!(
            sent.iter()
                .any(|c| matches!(c, crate::net::ClientCommand::CancelAutoRepeat)),
            "the cancel acks the server (CMSG_CANCEL_AUTO_REPEAT_SPELL)"
        );
    }

    /// `SMSG_REMOVED_SPELL` shrinks the book and dirties the feeds only on a real removal: a talent
    /// wipe sends removals for every rank of the class tree, learned or not.
    #[test]
    fn a_removal_shrinks_the_book_and_dirties_the_feeds() {
        let mut actions = PlayerActions::default();
        actions.spells.extend([14522, 14788, 14789]);

        let mut errors = UiErrorKeys::default();
        removed_spell(14788, &mut actions, None, &mut errors);
        assert!(!actions.spells.contains(&14788));
        assert!(actions.dirty);

        actions.dirty = false;
        removed_spell(14788, &mut actions, None, &mut errors);
        assert!(!actions.dirty, "a spell we never knew is not a repaint");
    }

    /// The unlearn line carries the bare name, with no rank, and each of its gates silences it.
    #[test]
    fn an_unlearn_announces_the_bare_name_and_four_things_silence_it() {
        let row = |attributes: u32, cast_ui: u32| benilla_formats::SpellDisplay {
            name: "Improved Fireball".to_string(),
            rank: Some("Rank 3".to_string()),
            attributes,
            cast_ui,
            ..Default::default()
        };

        let mut errors = UiErrorKeys::default();
        removed_spell(
            11069,
            &mut PlayerActions::default(),
            Some(&catalog_with(11069, row(0, 0))),
            &mut errors,
        );
        assert_eq!(errors.0.len(), 1);
        assert_eq!(errors.0[0].key, "ERR_SPELL_UNLEARNED_S");
        assert_eq!(
            errors.0[0].arg_s(),
            Some("Improved Fireball"),
            "no rank on the unlearn path"
        );

        // …and each gate on its own, every one of them silent.
        for (attributes, cast_ui, why) in [
            (
                0x20,
                0,
                "IS_TRADESKILL — the container join zeroes the flag",
            ),
            (
                0,
                1,
                "castUI > 0 takes the container walk and never reaches",
            ),
            (0x80, 0, "DO_NOT_DISPLAY — the sign test"),
        ] {
            let mut errors = UiErrorKeys::default();
            removed_spell(
                11069,
                &mut PlayerActions::default(),
                Some(&catalog_with(11069, row(attributes, cast_ui))),
                &mut errors,
            );
            assert!(errors.0.is_empty(), "{why}");
        }

        // An unknown id says nothing, like every other display path here.
        let mut errors = UiErrorKeys::default();
        removed_spell(
            99999,
            &mut PlayerActions::default(),
            Some(&catalog_with(11069, row(0, 0))),
            &mut errors,
        );
        assert!(errors.0.is_empty());
    }

    /// `castUI` gates the unlearn line but not the learn line, which reads it only afterwards, for
    /// the book slot.
    #[test]
    fn cast_ui_silences_the_unlearn_but_not_the_learn() {
        let spells = catalog_with(
            1234,
            benilla_formats::SpellDisplay {
                name: "Some Castbar Spell".to_string(),
                cast_ui: 2,
                ..Default::default()
            },
        );

        let mut errors = UiErrorKeys::default();
        learned_spell(
            1234,
            &mut PlayerActions::default(),
            Some(&spells),
            &mut errors,
            &mut LearnedInTab::default(),
            live(),
            &no_bar(),
            &Default::default(),
        );
        assert_eq!(errors.0.len(), 1, "the learn block never reads castUI");
        assert_eq!(errors.0[0].key, "ERR_LEARN_SPELL_S");

        let mut errors = UiErrorKeys::default();
        removed_spell(
            1234,
            &mut PlayerActions::default(),
            Some(&spells),
            &mut errors,
        );
        assert!(errors.0.is_empty(), "…but the unlearn block does");
    }
    /// The GO-deferred auto-attack start:
    /// - an Ex2 bit-20 spell's own GO swings at its first hit target;
    /// - Serpent Sting carries Ex2 bit 17 (`DO_NOT_RESET_COMBAT_TIMERS`), not bit 20: no swing;
    /// - already swinging, nothing;
    /// - another caster's Backstab, nothing.
    #[test]
    fn a_go_deferred_spell_swings_at_its_first_hit_and_the_hunter_shots_do_not() {
        use crate::combat_text::CombatTextSpawn;
        use crate::creature_anim::{Casting, Engaged};
        use crate::go_anim::GoLidOpen;
        use crate::net::{ClientCommand, Guid, SelfPlayer};
        use bevy::ecs::system::RunSystemOnce;

        const BACKSTAB: u32 = 53;
        const SERPENT_STING: u32 = 1978;
        let spell = |name: &str, ex2: u32| benilla_formats::SpellDisplay {
            name: name.into(),
            attributes_ex2: ex2,
            ..Default::default()
        };
        let make_spells = || crate::ui_action::Spells {
            power_displays: Default::default(),
            difficulties: Default::default(),
            instance_difficulty: Default::default(),
            catalog: benilla_formats::SpellCatalog::from_displays(
                [
                    // The 1.12 rows: Backstab Ex2 0x100000, Serpent Sting Ex2 0x20000.
                    (BACKSTAB, spell("Backstab", 0x0010_0000)),
                    (SERPENT_STING, spell("Serpent Sting", 0x0002_0000)),
                ]
                .into_iter()
                .collect(),
            ),
            forms: Default::default(),
            ranges: Default::default(),
            cast_times: Default::default(),
            durations: Default::default(),
            radii: Default::default(),
        };

        // `caster` casts `spell_id`, landing on 20; `engaged` is our mirror of `[+0xc48]`.
        // Returns every command the seam put on the wire.
        let fire = |caster: u64, spell_id: u32, engaged: bool| -> Vec<ClientCommand> {
            let mut app = App::new();
            app.add_message::<CastEvent>()
                .add_message::<SpellGoTargets>()
                .add_message::<CombatTextSpawn>()
                .add_message::<GoLidOpen>()
                .add_message::<crate::creature_anim::SheathRequest>()
                .add_message::<crate::player::StandStateRequest>()
                .init_resource::<GuidIndex>()
                .init_resource::<SelfGuid>()
                .init_resource::<CastBarFeed>()
                .init_resource::<PendingCast>()
                .init_resource::<QueuedMeleeSpell>()
                .init_resource::<Cooldowns>()
                .init_resource::<crate::spell::SpellModifiers>()
                .init_resource::<crate::ui_pet::PetBar>()
                .init_resource::<crate::items::Items>();
            let self_e = app
                .world_mut()
                .spawn((Guid(10), SelfPlayer, ObjectStore::default()))
                .id();
            if engaged {
                app.world_mut().entity_mut(self_e).insert(Engaged(0));
            }
            let other_e = app
                .world_mut()
                .spawn((Guid(11), ObjectStore::default()))
                .id();
            {
                let mut index = app.world_mut().resource_mut::<GuidIndex>();
                index.0.insert(10, self_e);
                index.0.insert(11, other_e);
            }
            app.world_mut().resource_mut::<SelfGuid>().0 = Some(10);

            let (tx, rx) = crossbeam_channel::unbounded();
            let spells = make_spells();
            app.world_mut()
                .run_system_once(
                    move |mut commands: Commands,
                          index: Res<GuidIndex>,
                          casting: Query<&Casting>,
                          mut cast_events: MessageWriter<CastEvent>,
                          mut go_targets: MessageWriter<SpellGoTargets>,
                          self_guid: Res<SelfGuid>,
                          stores: Query<&mut ObjectStore>,
                          mut cast_bar: ResMut<CastBarFeed>,
                          mut pending: ResMut<PendingCast>,
                          mut queued_melee: ResMut<QueuedMeleeSpell>,
                          mut go_lid: MessageWriter<GoLidOpen>,
                          mut cooldowns: ResMut<Cooldowns>,
                          mut pet_bar: ResMut<crate::ui_pet::PetBar>,
                          mut items: ResMut<crate::items::Items>,
                          mut sheath: MessageWriter<crate::creature_anim::SheathRequest>| {
                        let net_commands = crate::net::NetCommands(tx.clone());
                        spell_go(
                            caster,
                            0,
                            spell_id,
                           0,
                            0,
                            vec![20],
                            vec![],
                            Vec::new(),
                            // A different guid from `hits[0]`, which the arm must take.
                            Some(99),
                            None,
                            (0, None, None, Vec::new(), true),
                            None,
                            None,
                            None,
                            &mut commands,
                            &index,
                            &casting,
                            &mut cast_events,
                            &mut go_targets,
                            &self_guid,
                            &stores,
                            &mut cast_bar,
                            &mut pending,
                            &mut queued_melee,
                            &mut go_lid,
                            &mut crate::ui_loot::LootLatch::default(),
                            (
                                &mut cooldowns,
                                Some(&spells),
                                &mut items,
                                &net_commands,
                                &mut pet_bar,
                                &crate::spell::SpellModifiers::default(),
                            ),
                            (
                                &mut crate::spell::AutoRepeatActive::default(),
                                &mut sheath,
                                engaged,
                            ),
                            1,
                        );
                    },
                )
                .unwrap();
            rx.try_iter().collect()
        };

        let swings = |cmds: &[ClientCommand]| {
            cmds.iter()
                .filter_map(|c| match c {
                    ClientCommand::AttackSwing { guid } => Some(*guid),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };

        assert_eq!(
            swings(&fire(10, BACKSTAB, false)),
            vec![20],
            "a bit20 spell's own GO starts the swing at its first hit target"
        );
        assert!(
            swings(&fire(10, SERPENT_STING, false)).is_empty(),
            "Serpent Sting carries Ex2 bit 17, not bit 20 — it starts nothing"
        );
        assert!(
            swings(&fire(10, BACKSTAB, true)).is_empty(),
            "already swinging: the reference's attack-lock gate refuses"
        );
        assert!(
            swings(&fire(11, BACKSTAB, false)).is_empty(),
            "somebody else's Backstab is not our attack-start"
        );
    }
    /// The `modalNextSpell` chain (the cast-result handler):
    /// - a successful sting chains Auto Shot;
    /// - a failed one does too;
    /// - the in-flight guard clears first, or the in-flight rung would refuse the chain;
    /// - Auto Shot already running sends nothing;
    /// - a reply for a spell not in flight chains nothing.
    #[test]
    fn a_hunter_shots_cast_result_chains_auto_shot_exactly_once() {
        use crate::net::{Guid, SelfPlayer};
        use bevy::ecs::system::RunSystemOnce;

        const SERPENT_STING: u32 = 1978;
        const AUTO_SHOT: u32 = 75;

        let spells = || crate::ui_action::Spells {
            power_displays: Default::default(),
            difficulties: Default::default(),
            instance_difficulty: Default::default(),
            catalog: benilla_formats::SpellCatalog::from_displays(
                [
                    (
                        SERPENT_STING,
                        benilla_formats::SpellDisplay {
                            name: "Serpent Sting".into(),
                            // The 1.12 row: ranged slot, and column 38 = 75.
                            attributes: 0x0001_0002,
                            attributes_ex2: 0x0002_0000,
                            modal_next_spell: AUTO_SHOT,
                            ..Default::default()
                        },
                    ),
                    (
                        AUTO_SHOT,
                        benilla_formats::SpellDisplay {
                            name: "Auto Shot".into(),
                            attributes: 0x0005_0012,
                            attributes_ex2: 0x20,
                            // Auto Shot's own column 38 is 0: the chain is one hop.
                            modal_next_spell: 0,
                            ..Default::default()
                        },
                    ),
                ]
                .into_iter()
                .collect(),
            ),
            forms: Default::default(),
            ranges: Default::default(),
            cast_times: Default::default(),
            durations: Default::default(),
            radii: Default::default(),
        };

        // One CAST_RESULT for `spell_id`, with `in_flight` outstanding and `running` the live
        // repeat. Returns (what the reply chains, whether the guard is still armed).
        let fire = |spell_id: u32, success: bool, in_flight: Option<u32>, running: Option<u32>| {
            let mut app = App::new();
            app.add_message::<CastEvent>()
                .init_resource::<GuidIndex>()
                .init_resource::<SelfGuid>()
                .init_resource::<CastErrors>()
                .init_resource::<crate::ui_action::UiErrorKeys>()
                .init_resource::<crate::ui_action::UiErrorKeys>()
                .init_resource::<CastBarFeed>()
                .init_resource::<PendingCast>()
                .init_resource::<QueuedMeleeSpell>()
                .init_resource::<Cooldowns>()
                .init_resource::<crate::spell::SpellModifiers>()
                .init_resource::<AutoRepeatActive>();
            let self_e = app.world_mut().spawn((Guid(10), SelfPlayer)).id();
            app.world_mut()
                .resource_mut::<GuidIndex>()
                .0
                .insert(10, self_e);
            app.world_mut().resource_mut::<SelfGuid>().0 = Some(10);
            app.world_mut().resource_mut::<AutoRepeatActive>().0 = running;
            if let Some(id) = in_flight {
                app.world_mut()
                    .resource_mut::<PendingCast>()
                    // `guards: false`: a ranged hunter shot is recorded but does not guard.
                    .arm(id, Instant::now(), false);
            }
            let (tx, _rx) = crossbeam_channel::unbounded();
            let cat = spells();
            let chained = app
                .world_mut()
                .run_system_once(
                    move |mut commands: Commands,
                          self_guid: Res<SelfGuid>,
                          index: Res<GuidIndex>,
                          mut cast_errors: ResMut<CastErrors>,
                          casting: Query<&Casting>,
                          mut cast_events: MessageWriter<CastEvent>,
                          mut pending: ResMut<PendingCast>,
                          mut queued_melee: ResMut<QueuedMeleeSpell>,
                          mut cooldowns: ResMut<Cooldowns>,
                          mut auto_repeat: ResMut<AutoRepeatActive>| {
                        let net = crate::net::NetCommands(tx.clone());
                        cast_result(
                            (spell_id, 0, 0),
                            success,
                            if success { None } else { Some(0x1b) },
                            [None; 2],
                            &mut commands,
                            &self_guid,
                            &index,
                            &mut cast_errors,
                            &casting,
                            &mut cast_events,
                            &mut pending,
                            &mut queued_melee,
                            &mut cooldowns,
                            &mut auto_repeat,
                            Some(&cat),
                            &net,
                            1,
                        )
                    },
                )
                .unwrap();
            let still_armed = app
                .world()
                .resource::<PendingCast>()
                .in_flight(Instant::now());
            (chained, still_armed)
        };

        // The ordinary case: the sting lands, and Auto Shot follows by itself.
        assert_eq!(
            fire(SERPENT_STING, true, Some(SERPENT_STING), None),
            (Some(AUTO_SHOT), false),
            "a successful sting chains Auto Shot, and clears the in-flight guard first"
        );
        // A failed one does too: both results converge on the same block.
        assert_eq!(
            fire(SERPENT_STING, false, Some(SERPENT_STING), None).0,
            Some(AUTO_SHOT),
            "a FAILED sting chains it as well"
        );
        // Already shooting: re-arm, never re-cast.
        assert_eq!(
            fire(SERPENT_STING, true, Some(SERPENT_STING), Some(AUTO_SHOT)).0,
            None,
            "Auto Shot already running: the equal branch sends nothing"
        );
        // Auto Shot's own replies terminate the chain.
        assert_eq!(
            fire(AUTO_SHOT, true, Some(AUTO_SHOT), Some(AUTO_SHOT)).0,
            None,
            "Auto Shot's own column 38 is 0 — no second hop"
        );
        // Not our in-flight cast (a proc's result, a stale reply): not ours to chain from.
        assert_eq!(
            fire(SERPENT_STING, true, Some(133), None).0,
            None,
            "a reply for a spell we do not have in flight chains nothing"
        );
    }

    fn slot(slot: u8, action: u32, kind: u8) -> ActionButton {
        ActionButton { slot, action, kind }
    }

    /// State 1 on a streamed player drops an unknown spell and an unlisted set (`0x6d882b`).
    #[test]
    fn a_set_table_bars_known_spells_and_listed_sets_only() {
        let mut actions = PlayerActions::default();
        actions.spells.insert(133);
        actions
            .equipment_sets
            .push(benilla_protocol::messages::EquipmentSet {
                guid: 1,
                id: 3,
                name: String::new(),
                icon: String::new(),
                items: [0; benilla_protocol::messages::EQUIPMENT_SET_SLOTS],
            });
        actions.locked = true;
        let table = vec![
            slot(0, 133, ACTION_KIND_SPELL),
            slot(1, 116, ACTION_KIND_SPELL),
            slot(2, 3, ACTION_KIND_EQUIPMENT_SET),
            slot(3, 4, ACTION_KIND_EQUIPMENT_SET),
            slot(4, 9, 0x40),
        ];
        let (tx, rx) = crossbeam_channel::unbounded();
        action_buttons(
            ACTION_BUTTONS_SET,
            table.clone(),
            true,
            &mut actions,
            &NetCommands(tx),
        );
        let mut barred: Vec<u8> = actions.buttons.keys().copied().collect();
        barred.sort_unstable();
        assert_eq!(barred, [0, 2, 4]);
        // Every other slot, empty ones included, is cleared on the server too.
        assert_eq!(rx.try_iter().count(), 144 - 3);
        assert!(!actions.locked, "state 1 unlocks");
        assert_eq!(actions.table, Some(table));
    }

    /// State 2 locks the writers; state 0 stores without barring, and the player's init bars a
    /// known spell and any set (`0x6e8000`).
    #[test]
    fn the_lock_refuses_writes_and_init_bars_the_stored_table() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let net = NetCommands(tx);
        let mut actions = PlayerActions::default();
        action_buttons(ACTION_BUTTONS_LOCK, Vec::new(), false, &mut actions, &net);
        assert!(!actions.locked, "no player, no lock");
        action_buttons(ACTION_BUTTONS_LOCK, Vec::new(), true, &mut actions, &net);
        actions.write_slot(5, 133, &net);
        assert!(actions.buttons.is_empty() && rx.try_iter().next().is_none());

        actions.spells.insert(133);
        let table = vec![
            slot(0, 133, ACTION_KIND_SPELL),
            slot(1, 4, ACTION_KIND_EQUIPMENT_SET),
            slot(2, 116, ACTION_KIND_SPELL),
        ];
        action_buttons(
            benilla_protocol::messages::ACTION_BUTTONS_INIT,
            table,
            true,
            &mut actions,
            &net,
        );
        assert!(actions.buttons.is_empty(), "state 0 only stores");

        let mut app = App::new();
        app.add_message::<crate::net::WorldEnterCascadeMessage>()
            .insert_resource(actions)
            .add_systems(Update, put_action_table_on_init);
        app.world_mut()
            .write_message(crate::net::WorldEnterCascadeMessage);
        app.update();
        let mut barred: Vec<u8> = app
            .world()
            .resource::<PlayerActions>()
            .buttons
            .keys()
            .copied()
            .collect();
        barred.sort_unstable();
        assert_eq!(barred, [0, 1]);
    }

    /// A logout and a second character with disjoint slots in one app: the bar is zeroed at the
    /// session end (`0x5a79d0`), so the second init bars its own table alone, and nothing is sent.
    #[test]
    fn a_character_switch_bars_only_the_new_characters_table() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let net = NetCommands(tx);
        let mut app = App::new();
        app.add_message::<crate::net::WorldEnterCascadeMessage>()
            .init_resource::<PlayerActions>()
            .add_systems(Update, put_action_table_on_init);
        let end = app.world_mut().register_system(on_session_end);
        let enter = |app: &mut App, spells: &[u32], table: Vec<ActionButton>| {
            {
                let mut actions = app.world_mut().resource_mut::<PlayerActions>();
                // AzerothCore's login sends state 1 before our player streams: stored, not barred.
                actions.spells = spells.iter().copied().collect();
                action_buttons(ACTION_BUTTONS_SET, table, false, &mut actions, &net);
            }
            app.world_mut()
                .write_message(crate::net::WorldEnterCascadeMessage);
            app.update();
            let mut barred: Vec<u8> = app
                .world()
                .resource::<PlayerActions>()
                .buttons
                .keys()
                .copied()
                .collect();
            barred.sort_unstable();
            barred
        };

        let mage = vec![
            slot(0, 133, ACTION_KIND_SPELL),
            slot(9, 66, ACTION_KIND_SPELL),
        ];
        assert_eq!(enter(&mut app, &[133, 66], mage), [0, 9]);
        app.world_mut().resource_mut::<PlayerActions>().dirty = false;

        app.world_mut()
            .run_system_with(
                end,
                SessionEvent::Disconnected {
                    reason: "logged out".into(),
                    end: benilla_protocol::SessionEnd::LoggedOut,
                },
            )
            .unwrap();
        let actions = app.world().resource::<PlayerActions>();
        assert!(actions.buttons.is_empty() && !actions.locked);
        assert!(actions.dirty, "the feed re-resolves the emptied slots");

        let warrior = vec![
            slot(72, 78, ACTION_KIND_SPELL),
            slot(84, 2457, ACTION_KIND_SPELL),
        ];
        assert_eq!(enter(&mut app, &[78, 2457], warrior), [72, 84]);
        assert_eq!(rx.try_iter().count(), 0, "no CMSG_SET_ACTION_BUTTON");
    }

    /// `0x801cc4`, `0x801d5e`: a zero, an infinite (`0xFFFFFFFF`) or an unstreamed caster's
    /// channel opens nothing, ours or another's.
    #[test]
    fn a_channel_with_no_positive_duration_or_no_unit_opens_nothing() {
        let t0 = Instant::now();
        let mut feed = CastBarFeed::default();
        let mut channel = ActiveChannel::default();
        channel_start(10, true, true, 10, 0, &mut channel, &mut feed);
        channel_start(10, true, true, 10, u32::MAX, &mut channel, &mut feed);
        channel_start(0x42, false, false, 10, 8_000, &mut channel, &mut feed);
        assert!(feed.0.is_empty());
        assert_eq!(channel.current(t0), None);
    }

    /// `0x804010`: the pet's guid picks the pet's list, any other guid the player's; only the
    /// cheat and `SMSG_SPELL_COOLDOWN` drop a stranger.
    #[test]
    fn event_clear_and_modify_route_a_stranger_to_the_player() {
        let mut player = Cooldowns::default();
        let mut pet = crate::ui_pet::PetBar::default();
        let t0 = Instant::now();
        player.start_recovery(1, 10_000, t0);
        pet_or_player_store(0xF140_0000_0000_0001, &mut player, &mut pet).clear_spell(1);
        assert_eq!(
            player.record_count(),
            0,
            "a stranger's clear reaches the player"
        );
        assert!(addressed_store(
            0xF140_0000_0000_0001,
            &SelfGuid(Some(10)),
            &mut player,
            &mut pet
        )
        .is_none());
    }

    /// `0x8071c0`: each book spell with Attributes `0x2`, lacking AttributesEx2 `0x20000` and
    /// AttributesEx4 `0x10000000`, gets a bare recovery; any other spell nothing.
    #[test]
    fn a_ranged_reset_starts_each_ranged_book_spells_recovery() {
        let ranged = |ex2: u32, ex4: u32| benilla_formats::SpellDisplay {
            attributes: 0x2,
            attributes_ex2: ex2,
            attributes_ex4: ex4,
            ..Default::default()
        };
        let spells = crate::ui_action::Spells {
            power_displays: Default::default(),
            difficulties: Default::default(),
            instance_difficulty: Default::default(),
            catalog: benilla_formats::SpellCatalog::from_displays(
                [
                    (75, ranged(0, 0)),
                    (76, ranged(0x2_0000, 0)),
                    (77, ranged(0, 0x1000_0000)),
                    (78, benilla_formats::SpellDisplay::default()),
                ]
                .into_iter()
                .collect(),
            ),
            forms: Default::default(),
            ranges: Default::default(),
            cast_times: Default::default(),
            durations: Default::default(),
            radii: Default::default(),
        };
        let actions = PlayerActions {
            spells: [75, 76, 77, 78].into_iter().collect(),
            ..Default::default()
        };
        let mut cds = Cooldowns::default();
        let summons = benilla_formats::SummonPropertiesCatalog::default();
        reset_ranged_combat_timer(500, &actions, Some(&spells), &summons, &mut cds);
        let now = Instant::now();
        let left =
            |cds: &Cooldowns, id| cds.info(id, 0, spells.catalog.get(id), now).remaining_ms > 0;
        assert_eq!(
            [75, 76, 77, 78].map(|id| left(&cds, id)),
            [true, false, false, false]
        );
        let mut cds = Cooldowns::default();
        reset_ranged_combat_timer(0, &actions, Some(&spells), &summons, &mut cds);
        assert_eq!(cds.record_count(), 0, "a zero time stores nothing");
    }

    /// `0x809bd5`-`0x809bf6`: a DONT_REPORT failure of a cooldown-on-event spell removes its
    /// parked record too; NOT_READY (shared 0x3c) keeps it.
    #[test]
    fn a_dont_report_failure_reverts_a_parked_on_event_record() {
        use bevy::ecs::system::RunSystemOnce;
        const STEALTH: u32 = 1784;
        let spells = crate::ui_action::Spells {
            power_displays: Default::default(),
            difficulties: Default::default(),
            instance_difficulty: Default::default(),
            catalog: benilla_formats::SpellCatalog::from_displays(
                [(
                    STEALTH,
                    benilla_formats::SpellDisplay {
                        recovery_ms: 10_000,
                        attributes: 0x0200_0000,
                        ..Default::default()
                    },
                )]
                .into_iter()
                .collect(),
            ),
            forms: Default::default(),
            ranges: Default::default(),
            cast_times: Default::default(),
            durations: Default::default(),
            radii: Default::default(),
        };
        let mut app = App::new();
        app.add_message::<CastEvent>()
            .init_resource::<GuidIndex>()
            .init_resource::<SelfGuid>()
            .init_resource::<CastErrors>()
            .init_resource::<PendingCast>()
            .init_resource::<QueuedMeleeSpell>()
            .init_resource::<Cooldowns>()
            .init_resource::<AutoRepeatActive>()
            .insert_resource(spells);
        let fail = |app: &mut App, reason: u8| {
            let (tx, _rx) = crossbeam_channel::unbounded();
            let mods = crate::spell::SpellModifiers::default();
            app.world_mut()
                .run_system_once(
                    move |mut commands: Commands,
                          self_guid: Res<SelfGuid>,
                          index: Res<GuidIndex>,
                          mut cast_errors: ResMut<CastErrors>,
                          casting: Query<&Casting>,
                          mut cast_events: MessageWriter<CastEvent>,
                          mut pending: ResMut<PendingCast>,
                          mut queued_melee: ResMut<QueuedMeleeSpell>,
                          mut cooldowns: ResMut<Cooldowns>,
                          mut auto_repeat: ResMut<AutoRepeatActive>,
                          spells: Res<Spells>| {
                        let row = spells.catalog.get(STEALTH);
                        cooldowns.apply_wire_cooldown(STEALTH, 0, 0, row, &mods, Instant::now());
                        assert!(cooldowns.has_on_hold_record(STEALTH, 0, 0));
                        cast_result(
                            (STEALTH, 0, 0),
                            false,
                            Some(reason),
                            [None; 2],
                            &mut commands,
                            &self_guid,
                            &index,
                            &mut cast_errors,
                            &casting,
                            &mut cast_events,
                            &mut pending,
                            &mut queued_melee,
                            &mut cooldowns,
                            &mut auto_repeat,
                            Some(&spells),
                            &crate::net::NetCommands(tx.clone()),
                            1,
                        );
                    },
                )
                .unwrap();
            let parked = app
                .world()
                .resource::<Cooldowns>()
                .has_on_hold_record(STEALTH, 0, 0);
            app.world_mut().resource_mut::<Cooldowns>().wipe();
            parked
        };
        assert!(!fail(&mut app, 0x17), "DONT_REPORT reverts");
        assert!(fail(&mut app, 0x3c), "NOT_READY keeps the parked record");
    }

    /// `0x809c70`: `SMSG_SPELL_FAILURE` plays the first school with a `Resistances.dbc` row's
    /// fizzle on the caster, and our own ammo failure cancels Auto Shot with its send.
    #[test]
    fn a_spell_failure_fizzles_and_an_ammo_failure_cancels_auto_shot() {
        use crate::net::{Guid, SelfPlayer, ServerSoundMessage};
        use bevy::ecs::system::RunSystemOnce;
        const SHOT: u32 = 75;
        let spells = crate::ui_action::Spells {
            power_displays: Default::default(),
            difficulties: Default::default(),
            instance_difficulty: Default::default(),
            catalog: benilla_formats::SpellCatalog::from_displays(
                [(
                    SHOT,
                    benilla_formats::SpellDisplay {
                        // Holy and Fire: Holy (index 1) has no row here.
                        school_mask: 0x2 | 0x4,
                        ..Default::default()
                    },
                )]
                .into_iter()
                .collect(),
            )
            .with_fizzle_sounds([Some(1), None, Some(1103), None, None, None, None]),
            forms: Default::default(),
            ranges: Default::default(),
            cast_times: Default::default(),
            durations: Default::default(),
            radii: Default::default(),
        };
        let mut app = App::new();
        app.add_message::<ServerSoundMessage>()
            .init_resource::<GuidIndex>()
            .init_resource::<SelfGuid>()
            .init_resource::<AutoRepeatActive>()
            .insert_resource(spells);
        let me = app.world_mut().spawn((Guid(10), SelfPlayer)).id();
        let other = app.world_mut().spawn(Guid(0x42)).id();
        {
            let mut index = app.world_mut().resource_mut::<GuidIndex>();
            index.0.insert(10, me);
            index.0.insert(0x42, other);
        }
        app.world_mut().resource_mut::<SelfGuid>().0 = Some(10);
        app.world_mut().resource_mut::<AutoRepeatActive>().0 = Some(SHOT);
        let (tx, rx) = crossbeam_channel::unbounded();
        let fail = |app: &mut App, caster: u64, reason: u8| {
            let tx = tx.clone();
            app.world_mut()
                .run_system_once(
                    move |mut commands: Commands,
                          spells: Res<Spells>,
                          index: Res<GuidIndex>,
                          self_guid: Res<SelfGuid>,
                          mut auto_repeat: ResMut<AutoRepeatActive>,
                          mut sounds: MessageWriter<ServerSoundMessage>| {
                        spell_failure(
                            caster,
                            SHOT,
                            reason,
                            Some(&spells),
                            &index,
                            &self_guid,
                            &mut auto_repeat,
                            &mut commands,
                            &crate::net::NetCommands(tx.clone()),
                            &mut sounds,
                        );
                    },
                )
                .unwrap();
            app.world_mut()
                .resource_mut::<Messages<ServerSoundMessage>>()
                .drain()
                .map(|m| (m.sound_id, m.source))
                .collect::<Vec<_>>()
        };
        // Another unit's ammo failure: its fizzle, our Auto Shot untouched.
        assert_eq!(fail(&mut app, 0x42, 0x2f), vec![(1103, Some(other))]);
        assert_eq!(app.world().resource::<AutoRepeatActive>().0, Some(SHOT));
        // Our interrupt: the fizzle alone.
        assert_eq!(fail(&mut app, 10, 0x1e), vec![(1103, Some(me))]);
        assert_eq!(app.world().resource::<AutoRepeatActive>().0, Some(SHOT));
        // Our NEED_AMMO_POUCH: Auto Shot cancels and the server hears it.
        fail(&mut app, 10, 0x30);
        assert_eq!(app.world().resource::<AutoRepeatActive>().0, None);
        assert!(rx
            .try_iter()
            .any(|c| matches!(c, crate::net::ClientCommand::CancelAutoRepeat)));
        // An unstreamed caster: nothing.
        assert!(fail(&mut app, 0x99, 0x2f).is_empty());
    }
}
