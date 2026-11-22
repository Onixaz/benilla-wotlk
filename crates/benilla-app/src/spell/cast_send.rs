//! The one cast-send path: every spell and item use leaves through [`send_spell_cast`], which runs
//! the reference's cast attempt and its commit, the cast send, rung for rung.
//!
//! The rungs: the auto-repeat toggle-off (the reference's action-button handler, ahead of the cast
//! attempt; its order against the profession intercept is unobservable), the profession intercept,
//! dead or a ghost, the targeting abort, in-flight, reagents and totems, the equipped item, combo
//! points, the attack pick, target binding and range, then the validator (not-ready and GCD, power,
//! crowd control, mounted, water, moving, form), the deferred cast-arm refusal and the targeting
//! cursor. The commit tail follows: ranged stance, auto-repeat arm, the send, the auto-attack
//! start, the GCD. A refusal is local and pre-commit: no packet, no GCD, no pending arm, only the
//! red line.
//!
//! An item use takes the same ladder: the item use calls a wrapper that runs the cast attempt with
//! the item as its third argument. Three rungs fork on it, hence [`CastCommit`]: the not-ready rung
//! (the item cooldown and 0x28, else the spell cooldown and 0x3c), the power gate (items skip it),
//! and the opcode (`0xab` vs `0x12e`).

use std::time::Instant;

use benilla_protocol::messages::UseItemTarget;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use crate::items::Items;
use crate::net::{ClientCommand, NetCommands, Objects, SelfPlayer};

use super::{cast_target, rank_pick, validator, AutoRepeatActive};
use crate::ui_action::{reagent_totem_refusal, CastRefusals, Spells};

/// What the commit sends: the cast send branches on whether the pending-cast guid (the item's when
/// the cast attempt got one, else the caster's) is the caster's; the item arm sends
/// `CMSG_USE_ITEM`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CastCommit {
    /// `CMSG_CAST_SPELL` (0x12e).
    Spell,
    /// `CMSG_USE_ITEM` (0xab): the item's wire position and guid, plus a caller-bound GameObject.
    Item {
        bag_index: u8,
        slot: u8,
        /// The item's template entry: the not-ready rung queries the `(use_spell, entry)` record.
        entry: u32,
        /// The item's guid, which the body names after the spell (`0x80b2d1`).
        item_guid: u64,
        /// The item use's own target argument, set only for a key in a lock; it bypasses the
        /// binder, as the cast attempt's target resolve takes the pair it was passed.
        on_object: Option<u64>,
    },
    /// `CMSG_PET_CAST_SPELL` (0x1f0): a caster that is not the active player (`0x80b315`).
    Pet { caster: u64 },
}

impl CastCommit {
    /// Whether this commit carries an item: what every forked rung reads.
    pub(crate) fn is_item(self) -> bool {
        matches!(self, CastCommit::Item { .. })
    }

    /// The local not-ready reason for this commit (the validator's forked first rung).
    fn not_ready_reason(self) -> u8 {
        if self.is_item() {
            0x28
        } else {
            0x3c
        }
    }
}

/// A targeting click's bind as a cast's targets block, the same under every opcode.
fn bound_target(bound: TargetedBind) -> UseItemTarget {
    match bound {
        TargetedBind::Dest(dest) => UseItemTarget::Dest(dest),
        TargetedBind::Source(src) => UseItemTarget::Source(src),
        TargetedBind::Item(guid) => UseItemTarget::Item(guid),
        TargetedBind::TradeItem => UseItemTarget::TradeItem,
        TargetedBind::Object(guid) => UseItemTarget::Object(guid),
        TargetedBind::Unit(guid) => UseItemTarget::Unit(guid),
        TargetedBind::Corpse(guid, target) => UseItemTarget::Corpse(target, guid),
        TargetedBind::Glyph(socket) => UseItemTarget::Glyph(socket),
    }
}

/// The class byte the finisher rung tests (`0x80d57d`).
const CLASS_WARRIOR: u8 = 1;

/// A player's spell press held at the cast attempt's attack pick in a harness without the press's step queue; with it, the pick is a step of its own ([`attack_pick_step`]).
#[derive(Resource, Default)]
pub(crate) struct HeldForPick(Option<HeldCast>);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct HeldCast {
    pub(crate) spell_id: u32,
    /// The cast attempt's guid argument, the verb's explicit unit, which the pick tries before the
    /// selection.
    pub(crate) passed: Option<u64>,
    /// The attempt's record count, taken at its top (`0x80cda8`), which its rest keeps.
    pub(crate) count: u8,
}

/// Where a cast attempt enters its ladder: its top, below the targeting cursor's stop (`0x80d3ce`), or below the attack pick (`0x80d661`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Rung {
    Top,
    CursorStop,
    AttackPick,
}

/// Everything the cast-send path reads, as one [`SystemParam`]: [`CastLadder::send`] is the only
/// way into [`send_spell_cast`], for every caster surface.
#[derive(SystemParam)]
pub(crate) struct CastLadder<'w, 's> {
    pub(crate) commands: Res<'w, NetCommands>,
    pub(crate) self_player:
        Query<'w, 's, (Entity, Has<crate::creature_anim::Engaged>), With<SelfPlayer>>,
    pub(crate) spells: Option<Res<'w, Spells>>,
    pub(crate) items: Res<'w, Items>,
    /// `TotemCategory.dbc`, the pre-send check's tool leg; absent without client data.
    pub(crate) totem_categories: Option<Res<'w, crate::ui_totem::TotemCategories>>,
    /// The reagent check walks the bags through it; the item arms resolve an instance guid here.
    pub(crate) objects: Objects<'w, 's>,
    pub(crate) sheath: MessageWriter<'w, crate::creature_anim::SheathRequest>,
    pub(crate) ecs: Commands<'w, 's>,
    pub(crate) pending: ResMut<'w, crate::spell::PendingCast>,
    pub(crate) queued_melee: ResMut<'w, crate::spell::QueuedMeleeSpell>,
    pub(crate) cooldowns: ResMut<'w, crate::spell::Cooldowns>,
    /// The talent spell modifiers the power gate's cost goes through.
    pub(crate) spell_mods: Res<'w, crate::spell::SpellModifiers>,
    pub(crate) auto_repeat: ResMut<'w, AutoRepeatActive>,
    pub(crate) trade_skill_opens: ResMut<'w, crate::ui_tradeskill::TradeSkillOpens>,
    pub(crate) ground: ResMut<'w, super::targeting::SpellTargeting>,
    pub(crate) held_for_pick: ResMut<'w, HeldForPick>,
    /// The cast send's `UNIT_SPELLCAST_SENT`, fired by the caller inside the press (`0x80b556`).
    pub(crate) press: crate::ui_cast::PressSignals<'w, 's>,
    /// The pet's cooldown list and pending cast, which a cast as the pet reads; its writes are
    /// queued, as the script appliers share the bar.
    pub(crate) pet_bar: Option<Res<'w, crate::ui_pet::PetBar>>,
    /// `secureAbilityToggle`; a harness without it reads the default, on.
    pub(crate) secure_toggle: Option<Res<'w, super::SecureAbilityToggle>>,
    /// Where a pet cast whose missile flies its aim goes to be aimed and sent.
    /// Absent without the vehicle plugin, when such a cast is sent plain.
    pub(crate) trajectory_casts: Option<ResMut<'w, crate::vehicle::TrajectoryCasts>>,
    /// The weapon imbue's trade-window confirm, and the played clock and enchant table its bound
    /// test reads.
    pub(crate) spell_enchant: Option<ResMut<'w, crate::ui_socket::SpellEnchantConfirm>>,
    pub(crate) trade_window: (
        Option<Res<'w, crate::net::PlayedClock>>,
        Option<Res<'w, crate::items::Enchants>>,
    ),
    /// The critter arm of the active-spell test; absent in a harness without the spell plugin.
    pub(crate) summons: Option<Res<'w, super::SummonSlots>>,
    /// The area leg of the player's cast.
    pub(crate) area_gate: crate::area::CastAreaGate<'w>,
    /// The player's book, which the send's rank pick walks (`0x53d9d0`).
    pub(crate) actions: Option<Res<'w, crate::ui_action::PlayerActions>>,
    /// The pet list, whose head alone re-ranks a pet's cast.
    pub(crate) pet_list: Option<Res<'w, crate::ui_pet::PetList>>,
    /// The item use's requirement reads (`0x6dc3f0` at `0x708eb9`).
    pub(crate) use_reqs: crate::ui_items::feed::UseReqs<'w, 's>,
    /// Our passenger record, the active-spell test's vehicle (`[unit+0xf60]`).
    pub(crate) seat: Query<'w, 's, &'static crate::vehicle::Passenger, With<SelfPlayer>>,
}

/// What a targeting-cursor click bound: what the location bind or the target bind fills into the
/// standing flag word after the ladder has run.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum TargetedBind {
    /// The terrain click's point in WoW coords: the location bind's bit-6 arm.
    Dest(benilla_protocol::messages::TargetPoint),
    /// The terrain click bound to the source slot: the location bind's bit-5 arm, tested first;
    /// writes `SPELLCAST+0x30` and wire bit `0x0020`.
    Source(benilla_protocol::messages::TargetPoint),
    /// The bag or paper-doll click's item guid.
    Item(u64),
    /// The partner's not-traded trade slot (`0x80c5f0`).
    TradeItem,
    /// The world click's GameObject guid: a chest, door, vein or herb.
    Object(u64),
    /// The world click's unit guid.
    Unit(u64),
    /// The world click's corpse guid and the corpse bit the target bind's corpse arm bound.
    Corpse(u64, benilla_protocol::messages::CorpseTarget),
    /// The glyph frame's socket, 0-based: the glyph click's glyph index (`0x80c57b`).
    Glyph(u32),
}

impl CastLadder<'_, '_> {
    /// The click path's guard on an active toggle (`0x80da80`): with `secureAbilityToggle`
    /// on, a spell cast under 1.5 s ago keeps its aura, and the
    /// press does nothing.
    pub(crate) fn keeps_fresh_toggle(&self, spell_id: u32) -> bool {
        self.secure_toggle.as_deref().is_none_or(|s| s.0)
            && self
                .cooldowns
                .as_ref()
                .recently_cast(spell_id, 0, Instant::now())
    }

    /// The click paths' active-spell test on our own player (`0x802cb0`).
    pub(crate) fn is_active_spell(
        &self,
        spell_id: u32,
        store: Option<&crate::net::ObjectStore>,
        auras: Option<&crate::unit_auras::UnitAuras>,
    ) -> bool {
        let Some(spells) = self.spells.as_ref() else {
            return false;
        };
        let Some(d) = spells.catalog.get(spell_id) else {
            return false;
        };
        let none = benilla_formats::SummonPropertiesCatalog::default();
        let summons = self.summons.as_ref().map_or(&none, |s| &s.0);
        let critter = store.and_then(|s| s.0.unit_critter());
        // The seat's vehicle (`0x747ff0`), when its `UNIT_FIELD_CREATEDBY` is us (`0x802d42`).
        let me = store.and_then(|s| s.0.object_guid());
        let own_vehicle = self
            .seat
            .single()
            .ok()
            .and_then(|p| self.objects.object(p.vehicle))
            .filter(|v| me.is_some() && v.unit_created_by() == me)
            .and_then(|v| v.object_entry());
        crate::ui_action::toggle::is_active_spell(
            spell_id,
            d,
            auras,
            &spells.catalog,
            summons,
            critter,
            own_vehicle,
        )
    }

    /// Our aura cancel (`0x802f80`): the early form drop at the press's step, `CMSG_CANCEL_AURA` after its listeners (`0x80324d`, `0x803458`).
    pub(crate) fn cancel_aura(&mut self, spell_id: u32) {
        if self.press.raises_at_step() {
            self.press.aura_cancel(spell_id);
        } else {
            let _ = self.commands.0.send(ClientCommand::CancelAura { spell_id });
        }
    }

    /// Sends the aura cancel (`0x802f80`) for an active spell on our own player.
    pub(crate) fn cancel_active_spell(
        &mut self,
        spell_id: u32,
        store: Option<&crate::net::ObjectStore>,
        auras: Option<&crate::unit_auras::UnitAuras>,
    ) {
        let (Some(spells), Some(store)) = (self.spells.as_ref(), store) else {
            return;
        };
        let none = benilla_formats::SummonPropertiesCatalog::default();
        let summons = self.summons.as_ref().map_or(&none, |s| &s.0);
        let sends = crate::ui_companion::dismiss(spell_id, &spells.catalog, summons, store, auras);
        for send in sends {
            match send {
                ClientCommand::CancelAura { spell_id } => self.cancel_aura(spell_id),
                send => {
                    let _ = self.commands.0.send(send);
                }
            }
        }
    }

    /// The send's rank pick (`0x53d9d0`) for a cast at `unit` by the player (`caster` `None`) or by
    /// the pet `caster`, given the caster's assist check toward the unit.
    pub(crate) fn rank_for(
        &self,
        spell_id: u32,
        caster: Option<u64>,
        unit: Option<&benilla_protocol::messages::ObjectFields>,
        assists: bool,
    ) -> u32 {
        let (Some(spells), Some(unit), true) = (self.spells.as_deref(), unit, assists) else {
            return spell_id;
        };
        let summoner = unit
            .unit_summoned_by()
            .and_then(|guid| self.objects.object(guid));
        match caster {
            None => {
                let Some(actions) = self.actions.as_deref() else {
                    return spell_id;
                };
                let none = benilla_formats::SummonPropertiesCatalog::default();
                let book = rank_pick::Book::Player {
                    spells: &actions.spells,
                    summons: self.summons.as_ref().map_or(&none, |s| &s.0),
                };
                book.pick(spell_id, &spells.catalog, unit, summoner)
            }
            // A caster other than the player re-ranks only as the pet list's head.
            Some(pet) => {
                let head = self
                    .pet_list
                    .as_deref()
                    .and_then(crate::ui_pet::PetList::head);
                let (true, Some(bar)) = (head == Some(pet), self.pet_bar.as_deref()) else {
                    return spell_id;
                };
                rank_pick::Book::Pet(&bar.spells.spells).pick(
                    spell_id,
                    &spells.catalog,
                    unit,
                    summoner,
                )
            }
        }
    }

    /// The targeting cursor's commit: the ladder ran when the cursor went up, so the click owes
    /// only the cast send's tail (the packet, the pending arm, the GCD), then clears the word.
    pub(crate) fn commit_targeted(
        &mut self,
        spell_id: u32,
        commit: CastCommit,
        bound: TargetedBind,
    ) {
        // The click re-enters the cast attempt with the cursor's record and its count (`0x80cda4`).
        let count = self.ground.count();
        if let CastCommit::Pet { caster } = commit {
            self.commit_pet(spell_id, caster, bound_target(bound), count);
            self.ground.clear();
            return;
        }
        if let TargetedBind::Dest(p) | TargetedBind::Source(p) = bound {
            debug!(
                "spell: {spell_id} commits a point {:?} on transport {:#x}",
                p.pos, p.transport
            );
        }
        let target = match bound {
            TargetedBind::Unit(guid) => Some(guid),
            _ => None,
        };
        // The send clears the cursor's pending cast before it goes out (`0x80b4a2`).
        let prelude = self.ground.prelude().cloned();
        self.ground.clear();
        let cancel_auras = prelude
            .map(|p| p.send(&self.commands, &mut self.ecs, &mut self.press))
            .unwrap_or_default();
        let send = SendCommit {
            cancel_auras,
            packet: SendPacket::Cursor { commit, bound },
            // The item send joins the shared tail that raises `SENT` for the active player (`0x80b2dc` -> `0x80b348`, `0x80b514`).
            sent: Some(target),
            // The cursor's record goes on the list with the send (`0x80cda4`).
            record: match commit {
                CastCommit::Item {
                    entry, item_guid, ..
                } => crate::spell::PendingRecord {
                    item: Some(entry),
                    ..crate::spell::PendingRecord::player(spell_id, count, item_guid)
                },
                _ => crate::spell::PendingRecord::player(spell_id, count, 0),
            },
            on_next_swing: false,
            tail: SendTail {
                spell_id,
                caster: None,
                visuals: bound != TargetedBind::TradeItem,
                // The cursor's commit sends no attack start.
                attack_at: None,
                now: Instant::now(),
                attempt: false,
                count,
            },
            tail_at_step: true,
        };
        // The form drop's listeners run before the send (`0x80af88` -> `0x721d89`, `0x80b4ee`).
        if send.waits_for_the_drop() {
            self.press.send_commit(send);
            return;
        }
        let tail = commit_send(
            send,
            &self.commands,
            self.pending.as_mut(),
            self.queued_melee.as_mut(),
            &mut self.press,
        );
        if let Some(tail) = tail {
            self.finish_send(tail);
        }
    }

    /// The cast send's tail after its `UNIT_SPELLCAST_SENT` ([`SendTail`]).
    pub(crate) fn finish_send(&mut self, tail: SendTail) {
        finish_send(
            tail,
            self.spells.as_deref(),
            &self.objects,
            &self.self_player,
            &mut self.ecs,
            &self.commands,
            &mut self.sheath,
            self.auto_repeat.as_mut(),
            self.queued_melee.as_mut(),
            self.cooldowns.as_mut(),
            &self.spell_mods,
            &mut self.press,
        );
    }

    /// The spell-cast entry (`0x80cce0`) with the pet as caster, for the pet's client-targeted and
    /// possessed presses (`0x5d445a`). The rungs that read the casting unit run on the pet's
    /// [`cast_target::CastContext`]: a passive never casts, the dead rung, the per-caster pending
    /// bail (`0x80d34b`), the targeting abort, the target bind and its range, power, crowd control,
    /// then the targeting cursor or the send. The player's own state (bags, combo points,
    /// auto-repeat, the swing) is not the pet's and is not asked.
    pub(crate) fn send_pet(&mut self, spell_id: u32, ctx: &cast_target::CastContext, pet: u64) {
        self.send_pet_from(spell_id, ctx, pet, None);
    }

    /// [`Self::send_pet`], or with `rest` (the record's count) its rungs below the targeting cursor's stop alone.
    fn send_pet_from(
        &mut self,
        spell_id: u32,
        ctx: &cast_target::CastContext,
        pet: u64,
        rest: Option<u8>,
    ) {
        let now = Instant::now();
        let Some(d) = self.spells.as_ref().and_then(|s| s.catalog.get(spell_id)) else {
            return;
        };
        if d.attributes & 0x40 != 0 {
            return;
        }
        // The attempt files its record with the next count (`0x80cda8`).
        let count = rest.unwrap_or_else(|| self.pending.take_count());
        let rest = rest.is_some();
        if !rest && caster_dead_refusal(spell_id, Some(d), ctx.rel.self_store, &mut self.press) {
            return;
        }
        // The same spell pending for this caster bails on cooldown or `0x404` (`0x80d34b`, `0x80d36d`): the pet's history for the pet.
        if !rest && self.pending.listed_for(spell_id, pet) {
            let cooling = self
                .pet_bar
                .as_deref()
                .is_some_and(|bar| bar.cooldowns.not_ready(spell_id, 0, Some(d), now));
            if cooling || d.attributes & 0x404 != 0 {
                debug!("ui_pet: pet cast {spell_id} already pending — no packet");
                self.press.pending_bail();
                return;
            }
        }
        if !rest && self.ground.active() {
            let item = self.ground.cast_item();
            self.press.cast_item_cancelled(item);
            self.ground.clear();
            self.press.selection();
            // The rungs below run after the stop's selection update has (`0x80d3ce`).
            if self.press.raises_at_step() {
                self.press.ladder_rest(LadderRest {
                    spell_id,
                    commit: CastCommit::Pet { caster: pet },
                    bound_object: None,
                    hold: None,
                    candidate: ctx.selection_guid,
                    owner_casts: ctx.self_guid != Some(pet),
                    count,
                    from: Rung::CursorStop,
                });
                return;
            }
        }
        let candidates = cast_target::CastCandidates {
            selection: ctx.selection_guid,
            caster: ctx.self_guid,
            main_hand_item: None,
        };
        let mut pending_word = None;
        let mut deferred_refusal = None;
        let target = match cast_target::resolve_cast_target(
            Some(d),
            &candidates,
            ctx.auto_self_cast,
            &ctx.rel,
        ) {
            cast_target::CastWireTarget::SelfImplicit | cast_target::CastWireTarget::Item(_) => {
                None
            }
            cast_target::CastWireTarget::Unit(guid) => Some(guid),
            cast_target::CastWireTarget::Targeting(word) => {
                pending_word = Some(word);
                None
            }
            cast_target::CastWireTarget::Refused(reason) => {
                self.press.push_local(spell_id, reason);
                return;
            }
            cast_target::CastWireTarget::RefusedAtArm(reason) => {
                deferred_refusal = Some(reason);
                None
            }
        };
        if target.is_some() && target == ctx.selection_guid && target != ctx.self_guid {
            let row = self
                .spells
                .as_ref()
                .and_then(|s| s.ranges.get(d.range_index));
            if let Some(reason) = ctx.range.refusal(d, row, &self.spell_mods) {
                debug!("ui_pet: pet cast {spell_id} refused locally — range ({reason:#x})");
                self.press.push_local(spell_id, reason);
                return;
            }
        }
        if let Some(store) = ctx.rel.self_store {
            if !super::usable::can_afford(d, store, &self.spell_mods) {
                self.press.push_local(spell_id, 0x4d);
                return;
            }
        }
        let fields = ctx.rel.self_store.map(|s| &s.0);
        let catalog = self.spells.as_ref().map(|s| &s.catalog);
        if let Some((reason, mechanic)) = validator::cast_cc_refusal(
            fields.map_or(0, |f| f.unit_flags()),
            fields.and_then(|f| f.unit_health()),
            // The pet is charmed or summoned by us; the charm arm refuses only another's.
            false,
            Some(d),
            &mut |aura_types: &[u32]| {
                let ids = || {
                    ctx.self_auras
                        .into_iter()
                        .flat_map(|t| t.iter().map(|a| a.spell_id))
                };
                aura_types
                    .iter()
                    .map(|&ty| {
                        benilla_formats::cc_exemption(d, ids(), ty, |id| {
                            catalog.and_then(|c| c.get(id))
                        })
                    })
                    .reduce(|a, b| if a.mechanic != 0 || a.exempt { a } else { b })
                    .unwrap_or_default()
            },
        ) {
            match mechanic {
                Some(m) => self.press.push_local_arg(spell_id, reason, m),
                None => self.press.push_local(spell_id, reason),
            }
            return;
        }
        if let Some(reason) = deferred_refusal {
            self.press.push_local(spell_id, reason);
            return;
        }
        if let Some(word) = pending_word {
            debug!("ui_pet: pet cast {spell_id} awaits its click ({word:#06x})");
            self.ground
                .as_mut()
                .enter(spell_id, CastCommit::Pet { caster: pet }, word);
            self.ground.count_at_commit(count);
            self.press.selection();
            return;
        }
        // The send's rank pick (`0x53d9d0`); with `owner_casts` the player is the caster.
        let spell_id = match target {
            Some(unit) => {
                let is_self = ctx.self_guid == Some(unit);
                let store = if is_self {
                    ctx.rel.self_store
                } else {
                    ctx.rel.target_store
                };
                let caster = ctx.self_guid.filter(|&guid| guid == pet);
                let assists = ctx.rel.assistable(is_self);
                self.rank_for(spell_id, caster, store.map(|s| &s.0), assists)
            }
            None => spell_id,
        };
        let target = target.map_or(UseItemTarget::SelfImplicit, UseItemTarget::Unit);
        self.commit_pet(spell_id, pet, target, count);
    }

    /// The pending send's pet arm (`0x80b315`): `CMSG_PET_CAST_SPELL`, no `UNIT_SPELLCAST_SENT`
    /// (the player's only), the pet's pending cast and its cooldown start (the player's, with the
    /// pet as caster).
    fn commit_pet(&mut self, spell_id: u32, pet: u64, target: UseItemTarget, count: u8) {
        let flies_aim = self
            .spells
            .as_deref()
            .and_then(|s| s.catalog.get(spell_id))
            .is_some_and(|d| d.flies_aim);
        let aims = self.trajectory_casts.as_deref_mut().filter(|_| flies_aim);
        let aimed = aims.is_some();
        if let Some(aims) = aims {
            // `0x80b39d`: the aim replaces the targets, flown from the mover.
            debug!("ui_pet: pet cast {spell_id} as {pet:#x} flies an aim");
            aims.0.push(crate::vehicle::TrajectoryCast {
                caster: pet,
                spell_id,
                count,
            });
        } else {
            debug!("ui_pet: pet cast {spell_id} sent as {pet:#x} at {target:?}");
            let _ = self.commands.0.send(ClientCommand::PetCastSpell {
                count,
                pet_guid: pet,
                spell_id,
                target,
            });
        }
        // The pet's record, keyed on the pet (`0x80cda8`).
        self.pending.file(crate::spell::PendingRecord {
            pet: true,
            ..crate::spell::PendingRecord::player(spell_id, count, pet)
        });
        start_sent_cast(&mut self.ecs, Some(pet), spell_id, count, aimed);
        let now = Instant::now();
        let mods = self.spell_mods.clone();
        let speed = cast_speed(self.objects.object(pet));
        self.ecs.queue(move |world: &mut World| {
            if !world.contains_resource::<crate::ui_pet::PetBar>() {
                return;
            }
            let armed = world.resource_scope(|world, mut bar: Mut<crate::ui_pet::PetBar>| {
                let d = world
                    .get_resource::<crate::ui_action::Spells>()
                    .and_then(|s| s.catalog.get(spell_id));
                d.and_then(|d| {
                    bar.cooldowns
                        .start_gcd(spell_id, d, now, &mods, speed)
                        .then(|| d.cooldown_on_event())
                })
            });
            // The pet's armed start: the spellbook's signals, the usable update when on hold, then
            // the pet bar's (`0x805ec3`-`0x805eda`), fired as this send's commands apply.
            if let Some(on_hold) = armed {
                if let Some(mut ui) = world.get_resource_mut::<crate::ui_script::live::UiEvents>() {
                    crate::ui_action::signals::cooldown_site(
                        &mut ui,
                        on_hold,
                        crate::ui_action::signals::Tail::Pet,
                    );
                }
                crate::ui_script::live::fire_handler_events(world);
            }
        });
    }

    /// The cast attempt's dead rung alone, for the Attack button, whose short-circuit leaves the
    /// ladder right after it.
    pub(crate) fn dead_refusal(
        &mut self,
        spell_id: u32,
        self_store: Option<&crate::net::ObjectStore>,
    ) -> bool {
        let def = self.spells.as_ref().and_then(|s| s.catalog.get(spell_id));
        caster_dead_refusal(spell_id, def, self_store, &mut self.press)
    }

    /// Run the ladder for `spell_id` and commit as `commit` says, with no attack pick: no 1.12 item
    /// use, form, craft or `modalNextSpell` chain casts a spell of predicate.
    pub(crate) fn send(
        &mut self,
        spell_id: u32,
        ctx: &cast_target::CastContext,
        commit: CastCommit,
    ) {
        self.send_bound(spell_id, ctx, commit, None, None, None);
    }

    /// A player's spell press (`UseAction`, `CastSpell`, `CastSpellByName`): the ladder with the cast attempt's attack pick, a step of its own ([`attack_pick_step`]).
    pub(crate) fn send_spell(
        &mut self,
        spell_id: u32,
        ctx: &cast_target::CastContext,
        passed: Option<u64>,
    ) {
        // The count is filled in at the attempt's top.
        let held = HeldCast {
            spell_id,
            passed,
            count: 0,
        };
        self.send_bound(spell_id, ctx, CastCommit::Spell, None, Some(held), None);
    }

    /// The press held at the attack pick, if one is.
    pub(crate) fn take_held(&mut self) -> Option<HeldCast> {
        self.held_for_pick.0.take()
    }

    /// A held press after its pick, at the unit the pick left selected: the rungs below the pick, with the attempt's own count (`0x80d661`, `0x80d6ad`).
    pub(crate) fn resume_after_pick(&mut self, held: HeldCast, ctx: &cast_target::CastContext) {
        let from = Some((Rung::AttackPick, held.count));
        self.send_bound(held.spell_id, ctx, CastCommit::Spell, None, None, from);
    }

    /// The ladder for a cast the caller already bound to a GameObject (a lock opener): the cast
    /// attempt takes the guid pair it is passed, and the GameObject use-sender passes one.
    pub(crate) fn send_at_object(
        &mut self,
        spell_id: u32,
        ctx: &cast_target::CastContext,
        go_guid: u64,
    ) {
        self.send_bound(spell_id, ctx, CastCommit::Spell, Some(go_guid), None, None);
    }

    fn send_bound(
        &mut self,
        spell_id: u32,
        ctx: &cast_target::CastContext,
        commit: CastCommit,
        on_object: Option<u64>,
        hold: Option<HeldCast>,
        rest: Option<(Rung, u8)>,
    ) {
        // The attempt files its record with the next count (`0x80cda8`); its rest keeps it.
        let (from, count) = rest.unwrap_or_else(|| (Rung::Top, self.pending.take_count()));
        let played = self.trade_window.0.as_deref().map_or(0, |c| c.now());
        let enchants = self.trade_window.1.as_deref();
        let objects = &self.objects;
        let me = ctx.self_guid;
        let bound_trade = |item: u64| {
            objects
                .object(item)
                .is_some_and(|f| crate::ui_socket::bound_tradeable(f, me, enchants, played))
        };
        // The pressed spell's ranks in the book, taken here: a read of the lent book inside the
        // send would sit beside its owner writes (the cooldown store's), which refuse it.
        let ranks = match (commit, self.spells.as_deref(), self.actions.as_deref()) {
            (CastCommit::Spell, Some(spells), Some(known)) => {
                let none = benilla_formats::SummonPropertiesCatalog::default();
                let book = rank_pick::Book::Player {
                    spells: &known.spells,
                    summons: self.summons.as_ref().map_or(&none, |s| &s.0),
                };
                book.ranks(spell_id, &spells.catalog)
            }
            _ => None,
        };
        send_spell_cast(
            spell_id,
            ctx,
            commit,
            on_object,
            hold,
            (from, count),
            ranks.as_deref(),
            (self.spell_enchant.as_deref_mut(), &bound_trade),
            &self.commands,
            &self.self_player,
            self.spells.as_deref(),
            &self.objects,
            &self.items,
            self.totem_categories.as_deref().map(|t| &t.0),
            &mut self.sheath,
            &mut self.ecs,
            self.pending.as_mut(),
            self.queued_melee.as_mut(),
            self.cooldowns.as_mut(),
            &self.spell_mods,
            self.auto_repeat.as_mut(),
            &mut self.trade_skill_opens,
            self.ground.as_mut(),
            &mut self.held_for_pick,
            &self.area_gate,
            &mut self.press,
        );
    }
}

/// The cast attempt's dead rung, after the profession intercept and ahead of the Attack
/// short-circuit: a dead or ghost caster is refused with 0x13, "You are dead", unless the spell is
/// castable while dead (`Attributes & 0x800000`).
fn caster_dead_refusal(
    spell_id: u32,
    def: Option<&benilla_formats::SpellDisplay>,
    self_store: Option<&crate::net::ObjectStore>,
    cast_errors: &mut impl CastRefusals,
) -> bool {
    if !self_store.is_some_and(|s| s.0.is_dead_or_ghost())
        || def.is_some_and(|d| d.attributes & benilla_formats::ATTR_CASTABLE_WHILE_DEAD != 0)
    {
        return false;
    }
    debug!("ui_action: cast {spell_id} refused locally — dead or a ghost (0x13)");
    cast_errors.push_local(spell_id, 0x13);
    true
}

/// Run the ladder for one cast and commit it. The commit tail: a ranged spell arms the ranged
/// stance (the sheath state set to `(2,1,1)`; the echoed START re-requests it), an auto-repeat
/// spell sets the sticky armed state (`|= 0x200`, the Load/Hold idle's gate), and the packet goes
/// out.
///
/// The `pending` guard is the cast attempt's in-flight refusal ([`crate::spell::PendingCast`]): any
/// press while a guarding cast is in flight is refused locally. Ranged and on-next-swing commits
/// are recorded but never guard.
fn send_spell_cast(
    spell_id: u32,
    ctx: &cast_target::CastContext,
    commit: CastCommit,
    // A caller-bound GameObject for a spell commit; an item commit carries its own.
    bound_object: Option<u64>,
    // Set for a player's spell press, which parks in `held_for_pick` at the attack pick.
    hold: Option<HeldCast>,
    // The rung the attempt enters at (below the cursor's stop or the attack pick, each run after its listeners), and the attempt's record count.
    (from, count): (Rung, u8),
    // The pressed spell's ranks in the player's book ([`rank_pick::Book::ranks`]), when the send's
    // rank pick reads it.
    ranks: Option<&[&benilla_formats::SpellDisplay]>,
    // The weapon imbue's trade-window confirm and its bound test.
    trade_confirm: (
        Option<&mut crate::ui_socket::SpellEnchantConfirm>,
        &dyn Fn(u64) -> bool,
    ),
    commands: &NetCommands,
    self_player: &Query<(Entity, Has<crate::creature_anim::Engaged>), With<SelfPlayer>>,
    spells: Option<&Spells>,
    objects: &Objects,
    items: &Items,
    totem_categories: Option<&benilla_formats::TotemCategoryCatalog>,
    sheath: &mut MessageWriter<crate::creature_anim::SheathRequest>,
    ecs: &mut Commands,
    pending: &mut crate::spell::PendingCast,
    queued_melee: &mut crate::spell::QueuedMeleeSpell,
    cooldowns: &mut crate::spell::Cooldowns,
    spell_mods: &crate::spell::SpellModifiers,
    auto_repeat: &mut AutoRepeatActive,
    trade_skill_opens: &mut crate::ui_tradeskill::TradeSkillOpens,
    ground: &mut super::targeting::SpellTargeting,
    held_for_pick: &mut HeldForPick,
    area_gate: &crate::area::CastAreaGate,
    press: &mut crate::ui_cast::PressSignals,
) {
    let now = Instant::now();
    let def = spells.and_then(|s| s.catalog.get(spell_id));
    let top = from == Rung::Top;
    // The cast attempt's first branch (ahead of every gate): an `Effect[0] == TRADE_SKILL` cast
    // never reaches the wire; the crafting window opens instead.
    if top && def.is_some_and(|d| d.effects[0] == benilla_formats::SPELL_EFFECT_TRADE_SKILL) {
        debug!("ui_action: cast {spell_id} is a profession opener — the crafting book opens, no packet");
        trade_skill_opens.0.push(spell_id);
        return;
    }
    // The re-press toggle: pressing the running auto-repeat spell cancels it. The reference does
    // this in the action-button handler, before the cast attempt is called.
    if top && def.is_some_and(|d| d.auto_repeat()) && auto_repeat.0 == Some(spell_id) {
        debug!("ui_action: cast {spell_id} re-pressed — auto-repeat toggles off");
        let self_e = self_player.single().ok().map(|(e, _)| e);
        crate::creature_anim::cancel_auto_repeat_local(
            self_e,
            auto_repeat,
            ecs,
            commands,
            &mut *press,
        );
        return;
    }
    if top && caster_dead_refusal(spell_id, def, ctx.rel.self_store, &mut *press) {
        return;
    }
    // The pending bail above the cursor's stop (`0x80d34b`, `0x80d3ce`): this caster's same spell pending ends the press silently while it cools or is on-next-swing.
    if top {
        let caster = ctx.self_guid.unwrap_or(0);
        let listed =
            pending.listed_for(spell_id, caster) || queued_melee.current() == Some(spell_id);
        if listed
            && (cooldowns.not_ready(spell_id, 0, def, now)
                || def.is_some_and(|d| d.attributes & 0x404 != 0))
        {
            debug!("ui_action: cast {spell_id} suppressed — already pending");
            press.pending_bail();
            return;
        }
    }
    // The cast attempt's targeting check leg: a new press while the cursor is up clears the word,
    // with no packet, and continues down the ladder.
    if top && ground.active() {
        debug!("ui_action: cast {spell_id} supersedes the targeting cursor");
        press.cast_item_cancelled(ground.cast_item());
        ground.clear();
        // The cursor's cast ends, and its finish runs the selection update (`0x8054f0`).
        press.selection();
        // The rungs below run after that update's listeners have (`0x80d3ce`).
        if press.raises_at_step() {
            press.ladder_rest(LadderRest {
                spell_id,
                commit,
                bound_object,
                hold,
                candidate: ctx.selection_guid,
                owner_casts: false,
                count,
                from: Rung::CursorStop,
            });
            return;
        }
    }
    // The rungs above the attack pick ran before it, in the attempt this resumes.
    let above_pick = from < Rung::AttackPick;
    // An on-next-swing spell (`Attributes & 0x404`) also queues on the melee slot ([`crate::spell::QueuedMeleeSpell`]).
    let on_next_swing = def.is_some_and(|d| d.on_next_swing());
    // The reagent check (`0x8091d0`), before the validator: a missing totem, tool category or
    // reagent refuses with 0x78, 0x9c or 0x5c.
    let tools = totem_categories.map(|categories| crate::ui_action::ToolCheck {
        categories,
        items,
        commands,
    });
    if above_pick
        && reagent_totem_refusal(
            spell_id,
            def,
            ctx.rel.self_store,
            objects,
            tools,
            &mut *press,
        )
    {
        return;
    }
    // The equipped-item requirement, the same search the button greying and tooltip use. It sits
    // above the crowd-control rung, so a stunned caster without the weapon is told about the
    // weapon.
    if let Some(d) = def.filter(|_| above_pick) {
        if let Some(store) = ctx.rel.self_store {
            if !super::usable::equipped_item_fits_cached(d, store, objects, items) {
                let reason = super::usable::equipped_item_reason(d);
                debug!("ui_action: cast {spell_id} refused locally — equipped item ({reason:#x})");
                press.push_local(spell_id, reason);
                return;
            }
        }
    }
    // The finisher rung (`0x80d4e7`-`0x80d5fd`), above the attack pick: a finishing move
    // (`AttributesEx & 0x500000`) with no combo points on the cast's target refuses
    // `CASTER_AURASTATE` for a warrior (class byte 1, `0x80d57d`), else `NO_COMBO_POINTS`
    // (12340's 0x16 and 0x4e, the shared table's 0x12 and 0x46). The player's aura-262
    // exemption (`0x80d56a`) is not built.
    if let (Some(d), Some(store), true) = (def, ctx.rel.self_store, above_pick) {
        if d.needs_combo_points() && ctx.combo_points == 0 {
            let reason = if store.0.unit_class() == Some(CLASS_WARRIOR) {
                0x12
            } else {
                0x46
            };
            debug!("ui_action: cast {spell_id} refused locally — no combo points ({reason:#x})");
            press.push_local(spell_id, reason);
            return;
        }
    }
    // The cast attempt's attack pick: a spell of predicate, pressed while not attacking, goes
    // through the attack validator, which keeps a hostile selection or selects the nearest enemy.
    // It sits above the bind and the range gate, so an acquired target out of reach reads "Out of
    // range.".
    if let Some(held) = hold.filter(|_| above_pick) {
        let engaged = self_player.single().is_ok_and(|(_, engaged)| engaged);
        if def.is_some_and(|d| d.initiates_combat()) && !engaged {
            debug!("ui_action: cast {spell_id} holds for the attack pick");
            let held = HeldCast { count, ..held };
            // The pick runs the target setter, whose signals its listeners hear before the rungs below it run (`0x80d661` -> `0x72bdb0` -> `0x524bf0`, then `0x80d6ad`).
            if press.raises_at_step() {
                press.attack_pick(LadderRest {
                    spell_id,
                    commit,
                    bound_object,
                    hold: Some(held),
                    candidate: ctx.selection_guid,
                    owner_casts: false,
                    count,
                    from: Rung::AttackPick,
                });
            } else {
                held_for_pick.0 = Some(held);
            }
            return;
        }
    }
    // The target resolve picks the wire target from the spell's targeting constraints, never the
    // raw selection. A caller-bound GameObject skips the walk, as the cast attempt's resolve takes
    // a passed guid pair.
    let mut pending_word = None;
    let mut item_target = None;
    let mut deferred_refusal = None;
    let explicit_object = match commit {
        CastCommit::Item { on_object, .. } => on_object,
        CastCommit::Spell | CastCommit::Pet { .. } => bound_object,
    };
    let candidates = cast_target::CastCandidates {
        selection: ctx.selection_guid,
        caster: ctx.self_guid,
        // The reference resolves the candidate through `(typemask 1)`: a guid naming no live object
        // binds nothing and falls to the cursor.
        main_hand_item: ctx
            .main_hand_item
            .filter(|guid| objects.object(*guid).is_some()),
    };
    let target = match explicit_object {
        Some(_) => None,
        None => {
            match cast_target::resolve_cast_target(def, &candidates, ctx.auto_self_cast, &ctx.rel) {
                cast_target::CastWireTarget::SelfImplicit => None,
                cast_target::CastWireTarget::Unit(guid) => Some(guid),
                // The main-hand auto-pick: already bound, it commits in the item shape. Not a unit,
                // so the range gate skips it (the reference tests `SPELLCAST+0x14 & 0x8202`).
                cast_target::CastWireTarget::Item(guid) => {
                    item_target = Some(guid);
                    None
                }
                cast_target::CastWireTarget::Targeting(word) => {
                    // The cursor comes up, after the validator, so the word parks until the rungs
                    // pass. The pending-cast block (item guid included) survives the cursor, so the
                    // commit rides the word.
                    pending_word = Some(word);
                    None
                }
                cast_target::CastWireTarget::Refused(reason) => {
                    debug!(
                    "ui_action: cast {spell_id} refused locally — unbindable target ({reason:#x})"
                );
                    press.push_local(spell_id, reason);
                    return;
                }
                // The cast attempt's tail after the target resolve returns false runs below the
                // validator, so this parks and fires where the cursor would come up: a mounted
                // press with an empty weapon hand reads "You are mounted" first.
                cast_target::CastWireTarget::RefusedAtArm(reason) => {
                    deferred_refusal = Some(reason);
                    None
                }
            }
        }
    };
    // A weapon imbue whose item is bound but still in its trade window asks first and the cast is
    // released (`0x80cb23`); the Yes casts it at that item (`EndBoundTradeable("spellenchant")`).
    if let (Some(item), (Some(confirm), bound_trade)) = (item_target, trade_confirm) {
        if bound_trade(item) {
            debug!("ui_action: cast {spell_id} on {item:#x} ends its trade window — confirm first");
            confirm.parked = Some((spell_id, item));
            confirm.raise = true;
            return;
        }
    }
    // The range gate (the unit-target check → the range check, before the commit): a too-close shot
    // never reaches the sheath snap. Only the selection is tested; a self-bind is distance 0.
    if let Some(d) = def {
        if target.is_some() && target == ctx.selection_guid && target != ctx.self_guid {
            let row = spells.and_then(|s| s.ranges.get(d.range_index));
            if let Some(reason) = ctx.range.refusal(d, row, spell_mods) {
                debug!("ui_action: cast {spell_id} refused locally — range ({reason:#x})");
                press.push_local(spell_id, reason);
                return;
            }
        }
    }
    // ── The validator ──
    //
    // Not-ready: an item press queries (use-spell, entry) and refuses 0x28, a spell press queries
    // (spell, 0) and refuses 0x3c. The GCD rides the same query, matched on
    // `StartRecoveryCategory`. Local, because the server's NOT_READY fail clears the running GCD.
    let queried_item = match commit {
        CastCommit::Item { entry, .. } => entry,
        CastCommit::Spell | CastCommit::Pet { .. } => 0,
    };
    if cooldowns.not_ready(spell_id, queried_item, def, now) {
        debug!("ui_action: cast {spell_id} refused locally — not ready (the validator's rung 1)");
        // The one packet a local refusal sends (spell leg only): a running repeat with
        // AttributesEx3 0x400000 (wand Shoot) gets `CMSG_CANCEL_CAST`, then the local cancel.
        if !commit.is_item() {
            if let Some(cached) = auto_repeat.0 {
                if spells
                    .and_then(|s| s.catalog.get(cached))
                    .is_some_and(|d| d.casting_cancels_autorepeat())
                {
                    debug!("ui_action: the not-ready refusal cancels the wand repeat {cached}");
                    // The refusal's cancel writes count 0 (`0x8082ad`, `0x8082b1`).
                    let _ = commands.0.send(ClientCommand::CancelCast {
                        spell_id: cached,
                        count: 0,
                    });
                    let self_e = self_player.single().ok().map(|(e, _)| e);
                    crate::creature_anim::cancel_auto_repeat_local(
                        self_e,
                        auto_repeat,
                        ecs,
                        commands,
                        &mut *press,
                    );
                }
            }
        }
        press.push_local(spell_id, commit.not_ready_reason());
        return;
    }
    // The power gate (spells only: the item fork jumps past it): the power check `0x8017e0`
    // ([`super::usable::can_afford`]), refusing 0x4d. Local, because vmangos accepts the cast and
    // its NO_POWER fail clears the running GCD.
    if !commit.is_item() {
        if let (Some(d), Some(store)) = (def, ctx.rel.self_store) {
            if !super::usable::can_afford(d, store, spell_mods) {
                debug!("ui_action: cast {spell_id} refused locally — not enough power (0x4d)");
                press.push_local(spell_id, 0x4d);
                return;
            }
        }
    }
    // The validator's six crowd-control arms sit above its mounted block: a stunned mounted
    // caster reads the stun.
    let self_fields = ctx.rel.self_store.map(|s| &s.0);
    if let Some(reason) = validator::cast_cc_refusal(
        self_fields.map_or(0, |f| f.unit_flags()),
        self_fields.and_then(|f| f.unit_health()),
        // The charm arm refuses only when someone else holds the charm.
        self_fields
            .and_then(|f| f.unit_charmed_by())
            .is_some_and(|charmer| Some(charmer) != ctx.self_guid),
        def,
        // The exemption scan reads the caster's aura spell ids.
        &mut |aura_types: &[u32]| {
            let Some(d) = def else {
                return benilla_formats::CcExemption::default();
            };
            let ids = || {
                ctx.self_auras
                    .into_iter()
                    .flat_map(|t| t.iter().map(|a| a.spell_id))
            };
            let catalog = spells.as_ref().map(|s| &s.catalog);
            aura_types
                .iter()
                .map(|&ty| {
                    benilla_formats::cc_exemption(d, ids(), ty, |id| {
                        catalog.and_then(|c| c.get(id))
                    })
                })
                // The reference takes the first rejection and stops; exempt only on an accepted
                // match.
                .reduce(|a, b| if a.mechanic != 0 || a.exempt { a } else { b })
                .unwrap_or_default()
        },
    ) {
        let (reason, mechanic) = reason;
        debug!(
            "ui_action: cast {spell_id} refused locally — crowd control ({reason:#x}, \
             mechanic {mechanic:?})"
        );
        match mechanic {
            Some(m) => press.push_local_arg(spell_id, reason, m),
            None => press.push_local(spell_id, reason),
        }
        return;
    }
    // The mounted block (`0x739e8f`): unless `Attributes & 0x01000000`, a mounted caster the
    // auto-dismount check refuses reads 0x39 "You are mounted"; any other dismounts at the send.
    // The range gate runs first, so mounted and out of range reads "Out of range.".
    let dismount = validator::cast_mounted_refusal(
        ctx.rel
            .self_store
            .is_some_and(|s| s.0.unit_mount_display_id() > 0),
        def,
    );
    if dismount && !ctx.auto_dismount {
        debug!("ui_action: cast {spell_id} refused locally — mounted (0x39)");
        press.push_local(spell_id, 0x39);
        return;
    }
    // The seated block (`0x739650`): unless `Attributes & 0x08000000`, a seated caster the
    // auto-stand check refuses reads 0x3e "You have to be standing"; any other stands at the send.
    let stand = def.is_none_or(|d| d.attributes & 0x0800_0000 == 0)
        && ctx
            .rel
            .self_store
            .is_some_and(|s| s.0.unit_stand_state() != 0);
    if stand && !ctx.auto_stand {
        debug!("ui_action: cast {spell_id} refused locally — seated (0x3e)");
        press.push_local(spell_id, 0x3e);
        return;
    }
    // The water leg: 0x50 "Cannot use while swimming" for mounts, Travel Form and food, 0x58 "Can
    // only use while swimming" for Aquatic Form. Local, because vmangos's CheckCast has no such arm
    // and would grant Aquatic Form on land.
    if let Some(reason) = validator::cast_water_refusal(ctx.self_move_flags, def) {
        debug!("ui_action: cast {spell_id} refused locally — water side ({reason:#x})");
        press.push_local(spell_id, reason);
        return;
    }
    // The moving leg: a cast-time press while moving refuses 0x2e. Local, because vmangos accepts
    // the cast and then interrupts it mid-bar.
    if let Some(d) = def {
        let caster_level = ctx
            .rel
            .self_store
            .and_then(|s| s.0.unit_level())
            .unwrap_or(0);
        let cast_time_ms = spells.map_or(0, |s| s.cast_time_ms(d, caster_level, spell_mods));
        if validator::cast_moving_refusal(ctx.self_move_flags, cast_time_ms, def) {
            debug!("ui_action: cast {spell_id} refused locally — moving (0x2e)");
            press.push_local(spell_id, 0x2e);
            return;
        }
    }
    // The form leg (`0x72bac0`): needs-a-form always refuses, "Can't do that while shapeshifted"
    // only when the auto-unshift check does; otherwise the send leaves the form. A mount spell
    // leaves a form that changes the model too, a stance-flagged one included (`0x721da0`,
    // `0x726f30`).
    let mut cancel_form = false;
    let mut caster_form = 0;
    if let Some(d) = def {
        let form = ctx
            .rel
            .self_store
            .map(|s| s.0.unit_shapeshift_form())
            .unwrap_or(0);
        let form_flags = spells
            .and_then(|s| s.forms.get(&u32::from(form)))
            .map_or(0, |f| f.flags);
        caster_form = form;
        if let Some(refusal) = d.form_refusal(form, form_flags) {
            if matches!(refusal, benilla_formats::FormRefusal::OnlyShapeshift) || !ctx.auto_unshift
            {
                let reason = refusal.reason();
                debug!("ui_action: cast {spell_id} refused locally — the form gate ({reason:#x})");
                press.push_local(spell_id, reason);
                return;
            }
            cancel_form = true;
        }
        if d.effect_apply_aura
            .contains(&benilla_formats::SPELL_AURA_MOUNTED)
            && (validator::is_shapeshifted(form, form_flags)
                || (form != 0
                    && ctx
                        .rel
                        .self_store
                        .is_some_and(|s| s.0.unit_displayid() != s.0.unit_native_displayid())))
            && ctx.auto_unshift
        {
            cancel_form = true;
        }
    }
    let prelude = CastPrelude {
        stand,
        dismount,
        cancel_form: match (cancel_form, ctx.self_auras, spells) {
            (true, Some(auras), Some(spells)) => {
                crate::ui_shapeshift::form_auras(caster_form, auras, &spells.catalog)
            }
            _ => Vec::new(),
        },
    };
    // The aura-spell legs: the caster's (`0x739650`), then a bound unit's (`0x803500`).
    if let (Some(d), Some(sp)) = (def, spells) {
        let target_auras = match target {
            Some(guid) if ctx.self_guid == Some(guid) => ctx.self_auras,
            Some(_) => ctx.target_auras,
            None => None,
        };
        if let Some(reason) = validator::cast_aura_refusal(d, sp, ctx.self_auras, target_auras) {
            debug!("ui_action: cast {spell_id} refused locally — an aura spell ({reason:#x})");
            press.push_local(spell_id, reason);
            return;
        }
    }
    // The area leg (`0x739650`), after the form and aura-state legs.
    if let Some((reason, area)) = def.and_then(|d| area_gate.refusal(d)) {
        debug!("ui_action: cast {spell_id} refused locally — the area leg ({reason:#x})");
        match area {
            Some(area) => press.push_local_arg(spell_id, reason, area),
            None => press.push_local(spell_id, reason),
        }
        return;
    }
    // The place legs (`0x739650`), after the area leg.
    if let Some(reason) = def.and_then(|d| area_gate.place_refusal(d)) {
        debug!("ui_action: cast {spell_id} refused locally — the place leg ({reason:#x})");
        press.push_local(spell_id, reason);
        return;
    }
    // The deferred no-bind refusal: every rung passed and nothing bound.
    if let Some(reason) = deferred_refusal {
        debug!("ui_action: cast {spell_id} refused locally — the cast-arm tail ({reason:#x})");
        press.push_local(spell_id, reason);
        return;
    }
    // The deferred cursor entry: every rung passed, the cursor waits for its click.
    if let Some(word) = pending_word {
        debug!("ui_action: cast {spell_id} awaits its click — targeting cursor up ({word:#06x})");
        ground.enter(spell_id, commit, word);
        ground.prelude_at_commit(prelude);
        ground.count_at_commit(count);
        pending.mark_went(count);
        // The cursor's arm runs the selection update (`0x80d806`), then the attempt's repeat tail (`0x80d8f6` -> `0x80d983`).
        press.selection();
        let tail = SendTail {
            spell_id,
            caster: None,
            visuals: false,
            attack_at: None,
            now,
            attempt: true,
            count,
        };
        if press.raises_at_step() {
            press.tail(tail);
        } else {
            finish_send(
                tail,
                spells,
                objects,
                self_player,
                ecs,
                commands,
                sheath,
                auto_repeat,
                queued_melee,
                cooldowns,
                spell_mods,
                press,
            );
        }
        return;
    }
    if let Some(d) = def {
        if let Ok((e, _)) = self_player.single() {
            if d.ranged_attack() {
                sheath.write(crate::creature_anim::SheathRequest {
                    entity: e,
                    state: 2,
                    ceremony: false,
                });
            }
        }
    }
    // The send's rank pick (`0x53d9d0`), first in the send: a cast at a unit the caster may assist
    // goes out at the rank the unit can take, and everything from here names that rank.
    let pressed = spell_id;
    let spell_id = match (commit, target, explicit_object, item_target, ranks, spells) {
        (CastCommit::Spell, Some(unit), None, None, Some(ranks), Some(spells)) => {
            let is_self = ctx.self_guid == Some(unit);
            let store = if is_self {
                ctx.rel.self_store
            } else {
                ctx.rel.target_store
            };
            match store.filter(|_| ctx.rel.assistable(is_self)) {
                Some(store) => {
                    let summoner = store.0.unit_summoned_by().and_then(|g| objects.object(g));
                    rank_pick::pick_among(spell_id, &spells.catalog, ranks, &store.0, summoner)
                }
                None => spell_id,
            }
        }
        _ => spell_id,
    };
    if spell_id != pressed {
        debug!("ui_action: cast {pressed} goes out as {spell_id}, the rank its target takes");
    }
    // The pending send's stand, dismount and unshift go out ahead of the cast.
    let cancel_auras = prelude.send(commands, ecs, press);
    // Every commit files its record (`0x80cda8`), so a `UNIT_SPELLCAST_SENT` listener reads it: the pet's caster is the pet, an item use's the item (`0x805fc0`), else the player.
    let record = match commit {
        CastCommit::Item {
            entry, item_guid, ..
        } => crate::spell::PendingRecord {
            item: Some(entry),
            ..crate::spell::PendingRecord::player(spell_id, count, item_guid)
        },
        CastCommit::Pet { caster } => crate::spell::PendingRecord {
            pet: true,
            ..crate::spell::PendingRecord::player(spell_id, count, caster)
        },
        _ => crate::spell::PendingRecord::player(spell_id, count, ctx.self_guid.unwrap_or(0)),
    };
    let caster = match commit {
        CastCommit::Pet { caster } => Some(caster),
        _ => None,
    };
    // Spell and item sends join the tail raising `SENT` for the active player, not the pet (`0x80b2dc` -> `0x80b348`, `0x80b514`).
    let player_commit = !matches!(commit, CastCommit::Pet { .. });
    let send = SendCommit {
        cancel_auras,
        packet: SendPacket::Attempt {
            commit,
            explicit_object,
            item_target,
            target,
        },
        sent: player_commit.then_some(target),
        record,
        on_next_swing,
        tail: SendTail {
            spell_id,
            caster,
            visuals: true,
            attack_at: target,
            now,
            attempt: false,
            count,
        },
        tail_at_step: player_commit,
    };
    pending.mark_went(count);
    // The form drop's listeners run before the send (`0x80af88` -> `0x721d89`, `0x80b4ee`).
    if send.waits_for_the_drop() {
        press.send_commit(send);
        return;
    }
    if let Some(tail) = commit_send(send, commands, pending, queued_melee, press) {
        finish_send(
            tail,
            spells,
            objects,
            self_player,
            ecs,
            commands,
            sheath,
            auto_repeat,
            queued_melee,
            cooldowns,
            spell_mods,
            press,
        );
    }
}

/// The pending send's packet, built from what the ladder or the cursor's click bound.
#[derive(Clone, Copy, Debug, PartialEq)]
enum SendPacket {
    /// The attempt's own send: a caller-bound GameObject, the main-hand item, or the unit.
    Attempt {
        commit: CastCommit,
        explicit_object: Option<u64>,
        item_target: Option<u64>,
        target: Option<u64>,
    },
    /// The targeting cursor's commit, at what its click bound.
    Cursor {
        commit: CastCommit,
        bound: TargetedBind,
    },
}

impl SendPacket {
    /// The cast send: one targets block, two opcodes.
    fn command(self, spell_id: u32, count: u8) -> ClientCommand {
        match self {
            Self::Attempt {
                commit,
                explicit_object,
                item_target,
                target,
            } => match commit {
                CastCommit::Spell => match (explicit_object, item_target) {
                    // A lock opener bound by its click; the target bind's GameObject arm fills it.
                    (Some(go_guid), _) => ClientCommand::CastSpellGameObject {
                        count,
                        spell_id,
                        go_guid,
                    },
                    // The item leg, bound by the main-hand auto-pick.
                    (None, Some(item_guid)) => ClientCommand::CastSpellItem {
                        count,
                        spell_id,
                        item_guid,
                    },
                    (None, None) => ClientCommand::CastSpell {
                        count,
                        spell_id,
                        target,
                    },
                },
                CastCommit::Item {
                    bag_index,
                    slot,
                    item_guid,
                    on_object,
                    ..
                } => ClientCommand::UseItem {
                    count,
                    bag_index,
                    slot,
                    spell_id,
                    item_guid,
                    target: match (on_object, item_target, target) {
                        (Some(go), _, _) => UseItemTarget::Object(go),
                        (None, Some(item), _) => UseItemTarget::Item(item),
                        (None, None, Some(unit)) => UseItemTarget::Unit(unit),
                        (None, None, None) => UseItemTarget::SelfImplicit,
                    },
                },
                // The pet's casts take `CastLadder::send_pet`, never this ladder.
                CastCommit::Pet { caster } => ClientCommand::PetCastSpell {
                    count,
                    pet_guid: caster,
                    spell_id,
                    target: target.map_or(UseItemTarget::SelfImplicit, UseItemTarget::Unit),
                },
            },
            Self::Cursor { commit, bound } => match commit {
                // The same targets block under either opcode; one builder per bind. The pet's commit takes `commit_pet`.
                CastCommit::Spell | CastCommit::Pet { .. } => match bound {
                    TargetedBind::Dest(dest) => ClientCommand::CastSpellAtDest {
                        count,
                        spell_id,
                        dest,
                    },
                    TargetedBind::Source(src) => ClientCommand::CastSpellAtSource {
                        count,
                        spell_id,
                        src,
                    },
                    TargetedBind::Item(item_guid) => ClientCommand::CastSpellItem {
                        count,
                        spell_id,
                        item_guid,
                    },
                    TargetedBind::TradeItem => {
                        ClientCommand::CastSpellTradeItem { count, spell_id }
                    }
                    // The right-click OPEN_LOCK packet: the target bind's GameObject arm fills both.
                    TargetedBind::Object(go_guid) => ClientCommand::CastSpellGameObject {
                        count,
                        spell_id,
                        go_guid,
                    },
                    TargetedBind::Unit(unit_guid) => ClientCommand::CastSpell {
                        count,
                        spell_id,
                        target: Some(unit_guid),
                    },
                    TargetedBind::Corpse(corpse_guid, target) => ClientCommand::CastSpellCorpse {
                        count,
                        spell_id,
                        target,
                        corpse_guid,
                    },
                    TargetedBind::Glyph(_) => ClientCommand::CastSpellGlyph { count, spell_id },
                },
                CastCommit::Item {
                    bag_index,
                    slot,
                    item_guid,
                    ..
                } => ClientCommand::UseItem {
                    count,
                    bag_index,
                    slot,
                    spell_id,
                    item_guid,
                    target: bound_target(bound),
                },
            },
        }
    }
}

/// The pending send from its form drop on (`0x80324d`): the aura cancels (`0x803458`), `SENT` and the packet (`0x80b556`, `0x80b4ee`), the record and the tail, at a step after the drop's listeners.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SendCommit {
    /// The form's auras the drop cancels, sent after its listeners; empty when the drop did not raise at a step (the prelude sent them).
    cancel_auras: Vec<u32>,
    packet: SendPacket,
    /// `UNIT_SPELLCAST_SENT`'s unit, for a send that raises it.
    sent: Option<Option<u64>>,
    record: crate::spell::PendingRecord,
    /// An on-next-swing spell (`Attributes & 0x404`) also queues on the melee slot.
    on_next_swing: bool,
    tail: SendTail,
    /// The tail waits for `SENT`'s listeners at its own step.
    tail_at_step: bool,
}

impl SendCommit {
    /// The prelude's form drop raised at a step, so this waits for its listeners.
    fn waits_for_the_drop(&self) -> bool {
        !self.cancel_auras.is_empty()
    }
}

/// [`SendCommit`] in place: the tail it owes now, or `None` when the tail waits at its step.
fn commit_send(
    send: SendCommit,
    commands: &NetCommands,
    pending: &mut crate::spell::PendingCast,
    queued_melee: &mut crate::spell::QueuedMeleeSpell,
    press: &mut crate::ui_cast::PressSignals,
) -> Option<SendTail> {
    for &spell_id in &send.cancel_auras {
        let _ = commands.0.send(ClientCommand::CancelAura { spell_id });
    }
    let SendTail {
        spell_id, count, ..
    } = send.tail;
    if let Some(target) = send.sent {
        press.sent(spell_id, target);
    }
    let _ = commands.0.send(send.packet.command(spell_id, count));
    pending.file(send.record);
    if send.on_next_swing {
        queued_melee.arm(spell_id, count);
    }
    // After `UNIT_SPELLCAST_SENT` (`0x80b556`) the rest runs at the raise's step, once its listeners have run: the start visuals (`0x80b573`) and the GCD arm (`0x80b583`).
    if send.tail_at_step && press.raises_at_step() {
        press.tail(send.tail);
        return None;
    }
    Some(send.tail)
}

/// The send's commit at its step, after its form drop's listeners, with the `World` lent: its edges queue behind it on `ui`.
pub(crate) fn run_send_commit(
    In(send): In<SendCommit>,
    mut ladder: CastLadder,
    mut ui: ResMut<crate::ui_script::live::UiEvents>,
) {
    let tail = commit_send(
        send,
        &ladder.commands,
        ladder.pending.as_mut(),
        ladder.queued_melee.as_mut(),
        &mut ladder.press,
    );
    if let Some(tail) = tail {
        ladder.finish_send(tail);
    }
    ladder.press.defer(&mut ui);
}

/// What the cast send owes after `SENT` (`0x80b556`): the start visuals and the GCD arm (`0x80b573`, `0x80b583`), then the attempt's attack and repeat tail (`0x80d8ef`-`0x80d9fa`), each after the listeners before it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SendTail {
    spell_id: u32,
    /// The pet's guid for a pet commit, `None` the player.
    caster: Option<u64>,
    /// The trade-slot click sends with its visuals hidden (`0x80c62c`).
    visuals: bool,
    /// The unit the send named, which the attack tail starts the swing at.
    attack_at: Option<u64>,
    now: Instant,
    /// The attempt's attack and repeat tail alone, after the GCD arm's signals.
    attempt: bool,
    /// The attempt's record count, which the sent cast state keeps (`0x8053cd`).
    count: u8,
}

/// The send's tail at its step, with the `World` lent: the press's edges it raises queue behind it on `ui`.
pub(crate) fn run_send_tail(
    In(tail): In<SendTail>,
    mut ladder: CastLadder,
    mut ui: ResMut<crate::ui_script::live::UiEvents>,
) {
    ladder.finish_send(tail);
    ladder.press.defer(&mut ui);
}

/// A cast attempt's rungs below a raising step, the cursor's stop (`0x80d3ce`, `0x8054f0`) or the attack pick (`0x80d661`), run after its listeners against the unit it holds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LadderRest {
    spell_id: u32,
    commit: CastCommit,
    bound_object: Option<u64>,
    hold: Option<HeldCast>,
    candidate: Option<u64>,
    /// A pet's cast whose owner is the unit the ladder reads (`AttributesEx6 & 0x40000`).
    owner_casts: bool,
    /// The attempt's record count, taken at its top (`0x80cda8`).
    count: u8,
    /// The rung the rest enters at.
    from: Rung,
}

/// The attempt's rest at its step, with the `World` lent: its edges queue behind it on `ui`.
pub(crate) fn run_ladder_rest(
    In(rest): In<LadderRest>,
    targeting: cast_target::CastTargeting,
    mut ladder: CastLadder,
    mut ui: ResMut<crate::ui_script::live::UiEvents>,
) {
    match rest.commit {
        CastCommit::Pet { caster } => {
            let ctx = targeting.pet_context(caster, rest.candidate, rest.owner_casts);
            ladder.send_pet_from(rest.spell_id, &ctx, caster, Some(rest.count));
        }
        commit => {
            let ctx = targeting.context_at(cast_target::Aim::Unit(rest.candidate));
            ladder.send_bound(
                rest.spell_id,
                &ctx,
                commit,
                rest.bound_object,
                rest.hold,
                Some((rest.from, rest.count)),
            );
        }
    }
    ladder.press.defer(&mut ui);
}

/// The attack pick at its own step (`0x80d661` -> `0x72bdb0`): the setter's tail fires inside it (`0x524bf0`), the rungs below follow as a step; a failed pick ends the attempt (`0x80d67c`).
pub(crate) fn attack_pick_step(world: &mut World, rest: LadderRest) {
    let Some(held) = rest.hold else {
        return;
    };
    let picked = world.run_system_cached_with(run_attack_pick, held);
    let picked = match picked {
        Ok(picked) => picked,
        Err(e) => {
            crate::ui_cast::step_ran(Err(e), "a cast attempt's attack pick");
            None
        }
    };
    crate::ui_unit::target_set_tail(world);
    if let Some(unit) = picked {
        // The rest goes on at the unit the pick bound into the attempt's guid argument (`0x80d65e`), whatever the setter's listeners select.
        world
            .get_resource_or_init::<crate::ui_script::live::UiEvents>()
            .cast_edge(crate::ui_cast::CastBarEdge::LadderRest(LadderRest {
                hold: None,
                candidate: Some(unit),
                ..rest
            }));
    }
}

/// The pick, its signals and lines queued in turn on `ui` ahead of the rest it gates.
fn run_attack_pick(
    In(held): In<HeldCast>,
    mut attack: crate::ui_action::AttackPress,
    mut ui: ResMut<crate::ui_script::live::UiEvents>,
) -> Option<u64> {
    attack.pick_for_cast(held, &mut crate::script_calls::InTurn(&mut ui))
}

/// The send's tail: the start visuals and the GCD arm with its signals (`0x80b573`, `0x805d70`, `0x805ec3`), then [`attempt_tail`] after those listeners when the press raises at its step.
#[allow(clippy::too_many_arguments)]
fn finish_send(
    tail: SendTail,
    spells: Option<&Spells>,
    objects: &Objects,
    self_player: &Query<(Entity, Has<crate::creature_anim::Engaged>), With<SelfPlayer>>,
    ecs: &mut Commands,
    commands: &NetCommands,
    sheath: &mut MessageWriter<crate::creature_anim::SheathRequest>,
    auto_repeat: &mut AutoRepeatActive,
    queued_melee: &mut crate::spell::QueuedMeleeSpell,
    cooldowns: &mut crate::spell::Cooldowns,
    spell_mods: &crate::spell::SpellModifiers,
    press: &mut crate::ui_cast::PressSignals,
) {
    let def = spells.and_then(|s| s.catalog.get(tail.spell_id));
    if !tail.attempt {
        let SendTail {
            spell_id,
            caster,
            visuals,
            now,
            count,
            ..
        } = tail;
        if visuals {
            start_sent_cast(ecs, caster, spell_id, count, false);
        }
        // The GCD arms at send; a failed cast result clears it.
        if let Some(d) = def {
            let caster_fields = match caster {
                Some(pet) => objects.object(pet),
                None => objects.player_object(),
            };
            let armed =
                cooldowns.start_gcd(spell_id, d, now, spell_mods, cast_speed(caster_fields));
            debug!("spell: the send of {spell_id} finishes after its SENT (GCD armed: {armed})");
            // An armed start signals after the send's events (`0x805ec3`, `0x805ed1`).
            if armed {
                press.cooldown_start(d.cooldown_on_event());
                // The attempt's tail runs once the start's listeners have (`0x80d72c`).
                if press.raises_at_step() {
                    press.tail(SendTail {
                        attempt: true,
                        ..tail
                    });
                    return;
                }
            }
        }
    }
    attempt_tail(
        tail,
        def,
        spells,
        self_player,
        ecs,
        commands,
        sheath,
        auto_repeat,
        queued_melee,
        press,
    );
}

/// The attempt's tail after its send: the auto-attack start (`0x80d8ef`), then the repeat arm (`0x80d99b`) or a running wand repeat's end (`0x80d9b7`-`0x80d9fa`).
#[allow(clippy::too_many_arguments)]
fn attempt_tail(
    tail: SendTail,
    def: Option<&benilla_formats::SpellDisplay>,
    spells: Option<&Spells>,
    self_player: &Query<(Entity, Has<crate::creature_anim::Engaged>), With<SelfPlayer>>,
    ecs: &mut Commands,
    commands: &NetCommands,
    sheath: &mut MessageWriter<crate::creature_anim::SheathRequest>,
    auto_repeat: &mut AutoRepeatActive,
    queued_melee: &mut crate::spell::QueuedMeleeSpell,
    press: &mut crate::ui_cast::PressSignals,
) {
    let SendTail {
        spell_id,
        caster,
        attack_at,
        ..
    } = tail;
    // A spell that initiates auto-attack (`0x404` or `AttributesEx & 0x200`) starts the swing at its unit target (`0x80d8ef`-`0x80d97e`).
    if let (Some(d), Some(guid), None) = (def, attack_at, caster) {
        if d.initiates_auto_attack() {
            if let Ok((e, engaged)) = self_player.single() {
                // The tail's own gate (the attack lock `[player+0xc48]`, our `Engaged`) skips
                // entirely, so a strike pressed mid-swing leaves a running repeat alone. The attack
                // path's own test would still reach the repeat cancel; this route never gets there.
                if !engaged {
                    debug!("ui_action: cast {spell_id} initiates auto-attack at {guid:#x}");
                    // No stop is in flight: the gate above guarantees the lock is clear.
                    crate::creature_anim::start_attack_local(
                        e,
                        guid,
                        engaged,
                        false,
                        auto_repeat,
                        sheath,
                        ecs,
                        commands,
                        &mut *press,
                    );
                }
            }
        }
    }
    // `AttributesEx2 & 0x20` (`0x80d983`): the caster, when it is us, repeats this spell: `START_AUTOREPEAT_SPELL`, then the melee swing ends (`0x800a2e`, `0x800a5d`).
    if def.is_some_and(|d| d.auto_repeat()) {
        if caster.is_none() {
            if let Ok((e, engaged)) = self_player.single() {
                ecs.entity(e).insert(crate::creature_anim::AutoRepeatArmed);
                auto_repeat.set(spell_id, &mut *press);
                crate::creature_anim::stop_attack_local(engaged, queued_melee, commands);
            }
        }
        return;
    }
    // Any other cast ends a running repeat whose spell needs a wand (`0x80d9e3`, `0x80d9ec`): the repeat cancel with its send (`0x807560`), so Auto Shot survives.
    if let Some(cached) = auto_repeat.0 {
        if spells
            .and_then(|s| s.catalog.get(cached))
            .is_some_and(|d| d.requires_wand())
        {
            debug!("ui_action: cast {spell_id} cancels the running wand repeat {cached}");
            let self_e = self_player.single().ok().map(|(e, _)| e);
            crate::creature_anim::cancel_auto_repeat_local(
                self_e,
                auto_repeat,
                ecs,
                commands,
                &mut *press,
            );
            // Then every pending record of it ends (`0x8075e2` -> `0x806440` -> `0x806200`), each naming its own count (`0x8062de`).
            ecs.queue(move |world: &mut World| {
                let me = world
                    .get_resource::<crate::net::SelfGuid>()
                    .and_then(|g| g.0);
                let Some(me) = me else { return };
                let Some(mut pending) = world.get_resource_mut::<crate::spell::PendingCast>()
                else {
                    return;
                };
                let records: Vec<_> = pending
                    .release_spell(cached)
                    .into_iter()
                    .map(|r| (r, me))
                    .collect();
                crate::spell::cancel_records(world, &records, WAND_END_REASON);
            });
        }
    }
}

/// The wand end's cancel reason (`0x806467`).
const WAND_END_REASON: u8 = 0x1b;

/// A caster's `UNIT_MOD_CAST_SPEED`, the GCD arm's haste factor (`0x805e09`); 1.0 before it streams.
pub(crate) fn cast_speed(caster: Option<&benilla_protocol::messages::ObjectFields>) -> f32 {
    caster.and_then(|f| f.unit_mod_cast_speed()).unwrap_or(1.0)
}

/// What the pending send does ahead of the cast (`0x80ac90`), in its order.
#[derive(Clone, Default)]
pub(crate) struct CastPrelude {
    pub(crate) stand: bool,
    pub(crate) dismount: bool,
    /// The form's auras to cancel.
    pub(crate) cancel_form: Vec<u32>,
}

impl CastPrelude {
    /// Stand, dismount, then the form drop at a step, its aura cancels returned for the send's own step after the drop's listeners (`0x80324d` before `0x803458`); with no step queue they go out here.
    #[must_use]
    pub(crate) fn send(
        &self,
        net: &NetCommands,
        ecs: &mut Commands,
        press: &mut crate::ui_cast::PressSignals,
    ) -> Vec<u32> {
        if self.stand {
            ecs.write_message(crate::player::StandStateRequest { state: 0 });
        }
        if self.dismount {
            debug!("ui_action: the cast dismounts first (CMSG_CANCEL_MOUNT_AURA)");
            let _ = net.0.send(ClientCommand::CancelMountAura);
        }
        let at_step = press.raises_at_step();
        for &spell_id in &self.cancel_form {
            debug!("ui_action: the cast leaves the form first ({spell_id})");
            press.form_cancel(spell_id);
            if !at_step {
                let _ = net.0.send(ClientCommand::CancelAura { spell_id });
            }
        }
        if at_step {
            self.cancel_form.clone()
        } else {
            Vec::new()
        }
    }
}

/// The send's cast start (`0x80b573`): a caster holding no cast state takes a hidden one of the
/// sent spell and starts its precast, ahead of the server's start (`0x8053cd`). `caster` is the
/// pet's guid, `None` the player; `aimed` when the send flies a trajectory aim, whose instant
/// missile cast takes no state (`0x8053d6`, set at `0x80a09a`).
pub(crate) fn start_sent_cast(
    ecs: &mut Commands,
    caster: Option<u64>,
    spell_id: u32,
    count: u8,
    aimed: bool,
) {
    ecs.queue(move |world: &mut World| {
        let guid = caster.or(world
            .get_resource::<crate::net::SelfGuid>()
            .and_then(|g| g.0));
        let Some(entity) = guid.and_then(|g| {
            world
                .get_resource::<crate::net::GuidIndex>()
                .and_then(|i| i.0.get(&g).copied())
        }) else {
            return;
        };
        if world.get::<crate::creature_anim::Casting>(entity).is_some()
            || (aimed && sends_trajectory(world, entity, spell_id))
        {
            return;
        }
        world
            .entity_mut(entity)
            .insert(crate::creature_anim::Casting {
                spell_id,
                // The state is the pending record, count and all (`0x8053cd`).
                cast_count: count,
                until: None,
                listed: true,
                hidden: true,
            });
        let seq = world
            .get_resource_mut::<crate::creature_anim::PlaySeq>()
            .map_or(0, |mut s| s.next());
        if let Some(mut events) =
            world.get_resource_mut::<Messages<crate::creature_anim::CastEvent>>()
        {
            events.write(crate::creature_anim::CastEvent {
                entity,
                spell_id,
                kind: crate::creature_anim::CastEventKind::Sent,
                cast_count: 0,
                seq,
            });
        }
    });
}

/// The trajectory mark of a flown aim (`0x80a09a`): an instant cast whose visual has a missile
/// (`0x809fdf`, `0x80a065`).
fn sends_trajectory(world: &World, caster: Entity, spell_id: u32) -> bool {
    let Some(spells) = world.get_resource::<Spells>() else {
        return false;
    };
    let Some(def) = spells.catalog.get(spell_id) else {
        return false;
    };
    let level = world
        .get::<crate::net::ObjectStore>(caster)
        .and_then(|s| s.0.unit_level())
        .unwrap_or(0);
    let cast_ms = match world.get_resource::<crate::spell::SpellModifiers>() {
        Some(mods) => spells.cast_time_unclamped_ms(def, level, mods),
        None => spells.cast_time_unclamped_ms(def, level, &Default::default()),
    };
    let missile = world
        .get_resource::<crate::creature_anim::SpellVisuals>()
        .and_then(|v| {
            v.0.stages(crate::creature_anim::spell_visual::spell_visual_id(def))
                .map(|s| s.missile_gate != 0)
        })
        .unwrap_or(false);
    cast_ms <= 0 && missile
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::Reputations;
    use crate::ui_action::CastFail;
    use bevy::ecs::system::RunSystemOnce;
    use crossbeam_channel::Receiver;

    const HEARTHSTONE: u32 = 8690;
    const MOUNT: u32 = 470;
    /// The hearthstone's wire position on a fresh character: player array, backpack slot 1.
    const HEARTH_COMMIT: CastCommit = CastCommit::Item {
        bag_index: 255,
        slot: 24,
        entry: 6948,
        item_guid: HEARTH_GUID,
        on_object: None,
    };
    const HEARTH_GUID: u64 = 0x4000_0000_0000_0042;

    static EMPTY_REPUTATIONS: Reputations = Reputations(Vec::new(), Vec::new());

    /// A context with nothing selected: every rung that needs world state is inert.
    fn ctx() -> cast_target::CastContext<'static> {
        cast_target::CastContext {
            selection_guid: None,
            self_guid: Some(0x0000_0000_0000_0007),
            auto_self_cast: false,
            rel: cast_target::TargetRelations {
                target_store: None,
                target_owner_store: None,
                self_store: None,
                factions: None,
                reputations: &EMPTY_REPUTATIONS,
                types: Default::default(),
                group: Default::default(),
            },
            range: cast_target::RangeInputs::default(),
            main_hand_item: None,
            self_auras: None,
            target_auras: None,
            self_move_flags: 0,
            auto_dismount: true,
            auto_stand: true,
            auto_unshift: true,
            combo_points: 0,
        }
    }

    /// A World with exactly the resources [`CastLadder`] gathers and no `Spells`, so every rung
    /// that needs the spell's row is inert.
    fn world() -> (World, Receiver<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut world = World::new();
        world.insert_resource(NetCommands(tx));
        world.init_resource::<Items>();
        world.init_resource::<crate::net::GuidIndex>();
        world.init_resource::<crate::spell::PendingCast>();
        world.init_resource::<crate::spell::QueuedMeleeSpell>();
        world.init_resource::<crate::spell::Cooldowns>();
        world.init_resource::<crate::spell::SpellModifiers>();
        world.init_resource::<crate::ui_action::RefusalsSeen>();
        world.init_resource::<crate::ui_cast::PressEdges>();
        world.init_resource::<AutoRepeatActive>();
        world.init_resource::<crate::ui_tradeskill::TradeSkillOpens>();
        world.init_resource::<crate::spell::SpellTargeting>();
        world.init_resource::<crate::spell::HeldForPick>();
        // A bare World has no message storage until it is asked for.
        world.init_resource::<Messages<crate::creature_anim::SheathRequest>>();
        world.init_resource::<Messages<crate::player::StandStateRequest>>();
        (world, rx)
    }

    fn send(world: &mut World, spell_id: u32, commit: CastCommit) {
        world
            .run_system_once(move |mut ladder: CastLadder| {
                ladder.send(spell_id, &ctx(), commit);
            })
            .expect("the ladder runs as a one-shot system");
        crate::ui_cast::PressEdges::run_tails(world);
    }

    fn send_at_go(world: &mut World, spell_id: u32, go_guid: u64) {
        world
            .run_system_once(move |mut ladder: CastLadder| {
                ladder.send_at_object(spell_id, &ctx(), go_guid);
            })
            .expect("the ladder runs as a one-shot system");
        crate::ui_cast::PressEdges::run_tails(world);
    }

    /// The cast attempt's dead rung: a corpse and a ghost (health 1, the `PLAYER_FLAGS` bit) are
    /// refused with 0x13 and no packet, after the profession intercept; `Attributes & 0x800000`
    /// waives it, and the Attack button's own entry takes the same rung.
    #[test]
    fn a_dead_or_ghost_caster_is_refused_as_dead_unless_the_spell_waives_it() {
        use crate::net::ObjectStore;
        use benilla_formats::SpellDisplay;
        use benilla_protocol::ObjectFields;
        const PLAIN: u32 = 100;
        const WHILE_DEAD: u32 = 200;
        const PROFESSION: u32 = 300;
        const ATTACK: u32 = crate::ui_action::SPELL_ATTACK;

        let (mut world, rx) = world();
        let mut spells = crate::ui_action::Spells::empty_for_tests();
        spells.catalog = benilla_formats::SpellCatalog::from_displays(
            [
                (PLAIN, SpellDisplay::default()),
                (
                    WHILE_DEAD,
                    SpellDisplay {
                        attributes: benilla_formats::ATTR_CASTABLE_WHILE_DEAD,
                        ..Default::default()
                    },
                ),
                (
                    PROFESSION,
                    SpellDisplay {
                        effects: [benilla_formats::SPELL_EFFECT_TRADE_SKILL, 0, 0],
                        ..Default::default()
                    },
                ),
                // Attack's `Attributes` are `0x10`, no waiver.
                (
                    ATTACK,
                    SpellDisplay {
                        attributes: 0x10,
                        ..Default::default()
                    },
                ),
            ]
            .into_iter()
            .collect(),
        );
        world.insert_resource(spells);
        // Fields 22 health, 28 max health, 190 `PLAYER_FLAGS`. Leaked: the closure must be 'static.
        let leak = |pairs: &[(u16, u32)]| -> &'static ObjectStore {
            Box::leak(Box::new(ObjectStore(ObjectFields::from_pairs(pairs))))
        };
        let corpse = leak(&[
            (benilla_protocol::field::FIELD_UNIT_HEALTH, 0),
            (benilla_protocol::field::FIELD_UNIT_MAXHEALTH, 100),
        ]);
        let ghost = leak(&[
            (benilla_protocol::field::FIELD_UNIT_HEALTH, 1),
            (benilla_protocol::field::FIELD_UNIT_MAXHEALTH, 100),
            (benilla_protocol::field::FIELD_PLAYER_FLAGS, 0x10),
        ]);
        let press = |world: &mut World, spell_id: u32, store: &'static ObjectStore| {
            world
                .run_system_once(move |mut ladder: CastLadder| {
                    let base = ctx();
                    let with_store = cast_target::CastContext {
                        rel: cast_target::TargetRelations {
                            self_store: Some(store),
                            ..base.rel
                        },
                        ..base
                    };
                    ladder.send(spell_id, &with_store, CastCommit::Spell);
                })
                .expect("one-shot");
        };

        for (label, store) in [("corpse", corpse), ("ghost", ghost)] {
            press(&mut world, PLAIN, store);
            assert!(
                rx.try_recv().is_err(),
                "{label}: a refused press never sends"
            );
            assert_eq!(
                crate::ui_action::take_refusals(&mut world),
                vec![CastFail::local(PLAIN, 0x13)],
                "{label}: \"You are dead\""
            );
            let refused = world
                .run_system_once(move |mut ladder: CastLadder| {
                    ladder.dead_refusal(ATTACK, Some(store))
                })
                .expect("one-shot");
            assert!(refused, "{label}: the Attack button stops at the same rung");
            assert_eq!(
                crate::ui_action::take_refusals(&mut world),
                vec![CastFail::local(ATTACK, 0x13)]
            );
        }

        // The profession intercept comes first: the book opens, no error.
        press(&mut world, PROFESSION, ghost);
        assert_eq!(
            world.resource::<crate::ui_tradeskill::TradeSkillOpens>().0,
            vec![PROFESSION]
        );
        assert!(crate::ui_action::take_refusals(&mut world).is_empty());

        press(&mut world, WHILE_DEAD, ghost);
        assert!(
            matches!(rx.try_recv(), Ok(ClientCommand::CastSpell { spell_id, .. }) if spell_id == WHILE_DEAD),
            "the castable-while-dead attribute waives the rung"
        );
        assert!(crate::ui_action::take_refusals(&mut world).is_empty());
    }

    /// The mounted block (`0x739e8f`): a mounted press the auto-dismount check allows sends
    /// `CMSG_CANCEL_MOUNT_AURA` ahead of the cast (the pending send, `0x80af40`); one it refuses
    /// reads 0x39 with no packet; the `Attributes & 0x01000000` spell casts in the saddle.
    #[test]
    fn a_seated_cast_stands_first_unless_auto_stand_refuses() {
        use crate::net::ObjectStore;
        use benilla_protocol::ObjectFields;
        const SPELL: u32 = 100;
        let (mut world, rx) = world();
        let mut spells = crate::ui_action::Spells::empty_for_tests();
        spells.catalog = benilla_formats::SpellCatalog::from_displays(
            [(SPELL, benilla_formats::SpellDisplay::default())]
                .into_iter()
                .collect(),
        );
        world.insert_resource(spells);
        let seated: &'static ObjectStore =
            Box::leak(Box::new(ObjectStore(ObjectFields::from_pairs(&[
                (benilla_protocol::field::FIELD_UNIT_HEALTH, 100),
                (benilla_protocol::field::FIELD_UNIT_MAXHEALTH, 100),
                (benilla_protocol::field::FIELD_UNIT_BYTES_1, 1),
            ]))));
        for auto_stand in [true, false] {
            world
                .run_system_once(move |mut ladder: CastLadder| {
                    let base = ctx();
                    let seated_ctx = cast_target::CastContext {
                        rel: cast_target::TargetRelations {
                            self_store: Some(seated),
                            ..base.rel
                        },
                        auto_stand,
                        ..base
                    };
                    ladder.send(SPELL, &seated_ctx, CastCommit::Spell);
                })
                .expect("one-shot");
            world
                .resource_mut::<crate::spell::PendingCast>()
                .release_spell(SPELL);
            let cast = rx.try_iter().any(|c| {
                matches!(
                    c,
                    ClientCommand::CastSpell {
                        spell_id: SPELL,
                        ..
                    }
                )
            });
            let stood = world
                .resource_mut::<Messages<crate::player::StandStateRequest>>()
                .drain()
                .any(|r| r.state == 0);
            assert_eq!((cast, stood), (auto_stand, auto_stand));
        }
    }

    /// A cast its form refuses leaves the form first, unless the auto-unshift check says no.
    #[test]
    fn a_shapeshifted_cast_leaves_the_form_unless_auto_unshift_refuses() {
        use crate::net::ObjectStore;
        use benilla_formats::SpellDisplay;
        use benilla_protocol::messages::AURA_UPDATE_FLAG_CANCELABLE;
        use benilla_protocol::ObjectFields;
        const SPELL: u32 = 100;
        const FORM_AURA: u32 = 27545;
        let (mut world, rx) = world();
        let mut spells = crate::ui_action::Spells::empty_for_tests();
        spells.catalog = benilla_formats::SpellCatalog::from_displays(
            [
                (
                    SPELL,
                    SpellDisplay {
                        attributes: 0x0001_0000,
                        ..Default::default()
                    },
                ),
                (
                    FORM_AURA,
                    SpellDisplay {
                        shapeshift_form: Some(15),
                        ..Default::default()
                    },
                ),
            ]
            .into_iter()
            .collect(),
        );
        world.insert_resource(spells);
        let shifted: &'static ObjectStore =
            Box::leak(Box::new(ObjectStore(ObjectFields::from_pairs(&[
                (benilla_protocol::field::FIELD_UNIT_HEALTH, 100),
                (benilla_protocol::field::FIELD_UNIT_MAXHEALTH, 100),
                (benilla_protocol::field::FIELD_UNIT_BYTES_2, 15 << 24),
            ]))));
        let auras: &'static crate::unit_auras::UnitAuras = Box::leak(Box::new(
            crate::unit_auras::UnitAuras::of(1, &[(0, FORM_AURA, AURA_UPDATE_FLAG_CANCELABLE | 1)]),
        ));
        for auto_unshift in [true, false] {
            world
                .run_system_once(move |mut ladder: CastLadder| {
                    let base = ctx();
                    let shifted_ctx = cast_target::CastContext {
                        rel: cast_target::TargetRelations {
                            self_store: Some(shifted),
                            ..base.rel
                        },
                        self_auras: Some(auras),
                        auto_unshift,
                        ..base
                    };
                    ladder.send(SPELL, &shifted_ctx, CastCommit::Spell);
                })
                .expect("one-shot");
            crate::ui_cast::PressEdges::run_tails(&mut world);
            world
                .resource_mut::<crate::spell::PendingCast>()
                .release_spell(SPELL);
            let sent: Vec<ClientCommand> = rx.try_iter().collect();
            if auto_unshift {
                assert!(matches!(
                    sent.as_slice(),
                    [
                        ClientCommand::CancelAura {
                            spell_id: FORM_AURA
                        },
                        ClientCommand::CastSpell {
                            spell_id: SPELL,
                            ..
                        }
                    ]
                ));
            } else {
                assert!(sent.is_empty());
            }
        }
    }

    /// A mount spell leaves a stance-flagged form that changes the model, as Moonkin Form does.
    #[test]
    fn a_mount_spell_leaves_a_stance_flagged_form_with_a_model() {
        use crate::net::ObjectStore;
        use benilla_formats::{ShapeshiftForm, SpellDisplay, SPELL_AURA_MOUNTED};
        use benilla_protocol::messages::AURA_UPDATE_FLAG_CANCELABLE;
        use benilla_protocol::ObjectFields;
        const MOUNT: u32 = 100;
        const FORM_AURA: u32 = 24858;
        const FORM: u32 = 31;
        let (mut world, rx) = world();
        let mut spells = crate::ui_action::Spells::empty_for_tests();
        spells.catalog = benilla_formats::SpellCatalog::from_displays(
            [
                (
                    MOUNT,
                    SpellDisplay {
                        effect_apply_aura: [SPELL_AURA_MOUNTED, 0, 0],
                        ..Default::default()
                    },
                ),
                (
                    FORM_AURA,
                    SpellDisplay {
                        shapeshift_form: Some(FORM),
                        ..Default::default()
                    },
                ),
            ]
            .into_iter()
            .collect(),
        );
        spells.forms.insert(
            FORM,
            ShapeshiftForm {
                flags: 0xac1,
                ..Default::default()
            },
        );
        world.insert_resource(spells);
        let store: &'static ObjectStore =
            Box::leak(Box::new(ObjectStore(ObjectFields::from_pairs(&[
                (benilla_protocol::field::FIELD_UNIT_HEALTH, 100),
                (benilla_protocol::field::FIELD_UNIT_MAXHEALTH, 100),
                (benilla_protocol::field::FIELD_UNIT_DISPLAYID, 15374),
                (benilla_protocol::field::FIELD_UNIT_NATIVEDISPLAYID, 49),
                (benilla_protocol::field::FIELD_UNIT_BYTES_2, FORM << 24),
            ]))));
        let auras: &'static crate::unit_auras::UnitAuras = Box::leak(Box::new(
            crate::unit_auras::UnitAuras::of(1, &[(0, FORM_AURA, AURA_UPDATE_FLAG_CANCELABLE | 1)]),
        ));
        world
            .run_system_once(move |mut ladder: CastLadder| {
                let base = ctx();
                let form_ctx = cast_target::CastContext {
                    rel: cast_target::TargetRelations {
                        self_store: Some(store),
                        ..base.rel
                    },
                    self_auras: Some(auras),
                    ..base
                };
                ladder.send(MOUNT, &form_ctx, CastCommit::Spell);
            })
            .expect("one-shot");
        crate::ui_cast::PressEdges::run_tails(&mut world);
        let sent: Vec<ClientCommand> = rx.try_iter().collect();
        assert!(matches!(
            sent.as_slice(),
            [
                ClientCommand::CancelAura {
                    spell_id: FORM_AURA
                },
                ClientCommand::CastSpell {
                    spell_id: MOUNT,
                    ..
                }
            ]
        ));
    }

    #[test]
    fn a_mounted_cast_dismounts_first_unless_auto_dismount_refuses() {
        use crate::net::ObjectStore;
        use benilla_formats::SpellDisplay;
        use benilla_protocol::field::FIELD_UNIT_MOUNTDISPLAYID;
        use benilla_protocol::ObjectFields;
        const PLAIN: u32 = 100;
        const SADDLE: u32 = 200;

        let (mut world, rx) = world();
        let mut spells = crate::ui_action::Spells::empty_for_tests();
        spells.catalog = benilla_formats::SpellCatalog::from_displays(
            [
                (PLAIN, SpellDisplay::default()),
                (
                    SADDLE,
                    SpellDisplay {
                        attributes: 0x0100_0000,
                        ..Default::default()
                    },
                ),
            ]
            .into_iter()
            .collect(),
        );
        world.insert_resource(spells);
        let mounted: &'static ObjectStore =
            Box::leak(Box::new(ObjectStore(ObjectFields::from_pairs(&[
                (benilla_protocol::field::FIELD_UNIT_HEALTH, 100),
                (benilla_protocol::field::FIELD_UNIT_MAXHEALTH, 100),
                (FIELD_UNIT_MOUNTDISPLAYID, 2410),
            ]))));
        let press = |world: &mut World, spell_id: u32, auto_dismount: bool| {
            world
                .run_system_once(move |mut ladder: CastLadder| {
                    let base = ctx();
                    let mounted_ctx = cast_target::CastContext {
                        rel: cast_target::TargetRelations {
                            self_store: Some(mounted),
                            ..base.rel
                        },
                        auto_dismount,
                        ..base
                    };
                    ladder.send(spell_id, &mounted_ctx, CastCommit::Spell);
                })
                .expect("one-shot");
            world
                .resource_mut::<crate::spell::PendingCast>()
                .release_spell(spell_id);
            rx.try_iter().collect::<Vec<ClientCommand>>()
        };

        let sent = press(&mut world, PLAIN, true);
        assert!(
            matches!(
                sent.as_slice(),
                [
                    ClientCommand::CancelMountAura,
                    ClientCommand::CastSpell {
                        spell_id: PLAIN,
                        ..
                    }
                ]
            ),
            "the dismount goes first: {sent:?}"
        );
        assert!(crate::ui_action::take_refusals(&mut world).is_empty());

        assert!(
            press(&mut world, PLAIN, false).is_empty(),
            "refused, no packet"
        );
        assert_eq!(
            crate::ui_action::take_refusals(&mut world),
            vec![CastFail::local(PLAIN, 0x39)],
            "\"You are mounted\""
        );

        let sent = press(&mut world, SADDLE, false);
        assert!(
            matches!(
                sent.as_slice(),
                [ClientCommand::CastSpell {
                    spell_id: SADDLE,
                    ..
                }]
            ),
            "an allowed-while-mounted spell keeps the mount: {sent:?}"
        );
    }

    /// The click path's guard (`0x80da80`): a toggle cast under 1.5 s ago keeps its aura
    /// while `secureAbilityToggle` is on, and is free to cancel with it off or once the window
    /// has passed.
    #[test]
    fn a_fresh_toggle_keeps_its_aura_only_under_secure_ability_toggle() {
        use benilla_formats::SpellDisplay;
        const STEALTH: u32 = 1784;
        let (mut world, _rx) = world();
        let gcd = SpellDisplay {
            start_recovery_ms: 1000,
            ..Default::default()
        };
        world.resource_mut::<crate::spell::Cooldowns>().start_gcd(
            STEALTH,
            &gcd,
            Instant::now(),
            &crate::spell::SpellModifiers::default(),
            1.0,
        );
        let keeps = |world: &mut World| {
            world
                .run_system_once(|ladder: CastLadder| ladder.keeps_fresh_toggle(STEALTH))
                .expect("one-shot")
        };
        assert!(
            keeps(&mut world),
            "no resource reads the registered default, on"
        );
        world.insert_resource(crate::spell::SecureAbilityToggle(false));
        assert!(!keeps(&mut world), "off: the press cancels");
        world.insert_resource(crate::spell::SecureAbilityToggle(true));
        assert!(keeps(&mut world));
        let other = world
            .run_system_once(|ladder: CastLadder| ladder.keeps_fresh_toggle(STEALTH + 1))
            .expect("one-shot");
        assert!(!other, "another spell's cast does not guard this one");
    }

    /// A mashed opener with nothing cooling sends again (`0x80d36d` reads 0, `0x80d379` clear), and a different spell mid-opener too, with no local `0x61`.
    #[test]
    fn a_mashed_opener_with_nothing_cooling_sends_again() {
        const OPENING: u32 = 6478;
        const CHEST: u64 = 0xF110_0000_0000_1234;
        let (mut world, rx) = world();
        let opener = |rx: &Receiver<ClientCommand>| {
            rx.try_iter()
                .filter(|c| matches!(c, ClientCommand::CastSpellGameObject { spell_id, go_guid, .. } if *spell_id == OPENING && *go_guid == CHEST))
                .count()
        };
        send_at_go(&mut world, OPENING, CHEST);
        assert_eq!(opener(&rx), 1);
        send_at_go(&mut world, OPENING, CHEST);
        assert_eq!(opener(&rx), 1, "the re-click goes out");
        send(&mut world, MOUNT, CastCommit::Spell);
        assert!(rx.try_iter().any(|c| matches!(
            c,
            ClientCommand::CastSpell {
                spell_id: MOUNT,
                ..
            }
        )));
        assert!(crate::ui_action::take_refusals(&mut world).is_empty());
        assert_eq!(
            world
                .resource::<crate::spell::PendingCast>()
                .records()
                .len(),
            3,
            "one record per send"
        );
    }

    /// The same spell pending while it cools down ends the press silently (`0x80d34b`, `0x80d36d`): no packet, no line, the bail's sound edge alone (`0x80d385`).
    #[test]
    fn the_same_spell_pending_and_cooling_bails_silently() {
        let (mut world, rx) = world();
        world
            .resource_mut::<crate::spell::PendingCast>()
            .arm(MOUNT, 1);
        world
            .resource_mut::<crate::spell::Cooldowns>()
            .start_recovery(MOUNT, 30_000, Instant::now());
        send(&mut world, MOUNT, CastCommit::Spell);
        assert!(rx.try_recv().is_err(), "no packet");
        assert!(crate::ui_action::take_refusals(&mut world).is_empty());
        assert_eq!(
            world.resource::<crate::ui_cast::PressEdges>().waiting(),
            [crate::ui_cast::CastBarEdge::PendingBail]
        );
    }

    /// Cooling with nothing pending is the validator's not-ready rung, below the bail: `0x3c`.
    #[test]
    fn cooling_with_nothing_pending_reaches_the_not_ready_rung() {
        let (mut world, rx) = world();
        world
            .resource_mut::<crate::spell::PendingCast>()
            .arm(HEARTHSTONE, 1);
        world
            .resource_mut::<crate::spell::Cooldowns>()
            .start_recovery(MOUNT, 30_000, Instant::now());
        send(&mut world, MOUNT, CastCommit::Spell);
        assert!(rx.try_recv().is_err());
        assert_eq!(
            crate::ui_action::take_refusals(&mut world),
            vec![CastFail::local(MOUNT, 0x3c)],
            "a different spell pending is no bail; the cooldown refuses it"
        );
    }

    /// An answer naming another id releases nothing (`0x805180` keys on spell and count): the old record stays, and blocks no other spell.
    #[test]
    fn an_answer_naming_another_id_leaves_the_record_and_blocks_nothing() {
        const SENT_RANK: u32 = 1461;
        const CAST_RANK: u32 = 1459;
        let (mut world, rx) = world();
        world
            .resource_mut::<crate::spell::PendingCast>()
            .arm(SENT_RANK, 7);
        assert_eq!(
            world
                .resource_mut::<crate::spell::PendingCast>()
                .release(CAST_RANK, 7),
            None
        );
        send(&mut world, MOUNT, CastCommit::Spell);
        assert!(rx.try_iter().any(|c| matches!(
            c,
            ClientCommand::CastSpell {
                spell_id: MOUNT,
                ..
            }
        )));
        assert!(world
            .resource::<crate::spell::PendingCast>()
            .listed_for(SENT_RANK, 0));
    }

    // ── Melee and auto-repeat exclusion ──
    //
    // The auto-repeat commit calls StopAttack (the queued-cast cancel), and the attack-start's tail
    // cancels the running repeat.

    const AUTO_SHOT: u32 = 75;
    const SHOOT: u32 = 5019;
    const RAPTOR_STRIKE: u32 = 2973;
    const SERPENT_STING: u32 = 1978;
    const EVISCERATE: u32 = 2098;
    const MOB: u64 = 0x0000_0000_0000_00f1;

    /// Auto Shot (auto-repeat `AttributesEx2 & 0x20`, ranged `Attributes & 0x2`), Raptor Strike
    /// (on-next-swing `0x404`), Serpent Sting and Eviscerate (a finisher, `AttributesEx 0x100000`,
    /// that starts combat, `0x200`), as the 1.12 rows classify them. `Targets & 0x2` binds the
    /// selection without faction data.
    fn combat_catalog() -> Spells {
        use std::collections::HashMap;
        let auto_shot = benilla_formats::SpellDisplay {
            targets: 0x2,
            attributes: 0x2,
            attributes_ex2: 0x20,
            ..Default::default()
        };
        let raptor_strike = benilla_formats::SpellDisplay {
            targets: 0x2,
            attributes: 0x404,
            ..Default::default()
        };
        // An instant hunter shot: ranged, not auto-repeat, and it names Auto Shot in
        // `modalNextSpell`.
        let serpent_sting = benilla_formats::SpellDisplay {
            targets: 0x2,
            attributes: 0x0001_0002,
            attributes_ex2: 0x0002_0000,
            modal_next_spell: AUTO_SHOT,
            ..Default::default()
        };
        let eviscerate = benilla_formats::SpellDisplay {
            targets: 0x2,
            attributes_ex: 0x0010_0200,
            ..Default::default()
        };
        // Wand Shoot: auto-repeat, its equipped item a wand (class 2, subclass 19).
        let shoot = benilla_formats::SpellDisplay {
            targets: 0x2,
            attributes: 0x2,
            attributes_ex2: 0x20,
            equipped_item_class: 2,
            equipped_item_subclass_mask: 1 << 19,
            ..Default::default()
        };
        Spells {
            catalog: benilla_formats::SpellCatalog::from_displays(HashMap::from([
                (AUTO_SHOT, auto_shot),
                (SHOOT, shoot),
                (RAPTOR_STRIKE, raptor_strike),
                (SERPENT_STING, serpent_sting),
                (EVISCERATE, eviscerate),
            ])),
            ..Spells::empty_for_tests()
        }
    }

    const SENDER: u64 = 0x10;

    /// [`world`] with the player streamed in and the cast edges stored.
    fn sender_world() -> (World, Receiver<ClientCommand>, Entity) {
        let (mut world, rx) = world();
        world.init_resource::<Messages<crate::creature_anim::CastEvent>>();
        world.init_resource::<crate::creature_anim::PlaySeq>();
        let me = world.spawn(SelfPlayer).id();
        world.insert_resource(crate::net::SelfGuid(Some(SENDER)));
        world
            .resource_mut::<crate::net::GuidIndex>()
            .0
            .insert(SENDER, me);
        (world, rx, me)
    }

    fn sent_edges(world: &mut World) -> Vec<(u32, crate::creature_anim::CastEventKind)> {
        world
            .resource_mut::<Messages<crate::creature_anim::CastEvent>>()
            .drain()
            .map(|e| (e.spell_id, e.kind))
            .collect()
    }

    /// The send's cast start (`0x80b573`): a caster with no cast state takes a hidden one of the
    /// sent spell and starts its precast; one already holding a state keeps it, and no edge plays
    /// (`0x8053cd`).
    #[test]
    fn a_sent_cast_takes_the_free_cast_state_hidden_and_starts_its_precast() {
        use crate::creature_anim::{CastEventKind, Casting};
        let (mut world, _rx, me) = sender_world();
        send(&mut world, MOUNT, CastCommit::Spell);
        let held = *world.get::<Casting>(me).expect("the send set the state");
        assert_eq!(
            (held.spell_id, held.hidden, held.listed),
            (MOUNT, true, true)
        );
        assert_eq!(sent_edges(&mut world), [(MOUNT, CastEventKind::Sent)]);

        world.insert_resource(crate::spell::PendingCast::default());
        send(&mut world, HEARTHSTONE, CastCommit::Spell);
        assert_eq!(
            world.get::<Casting>(me).map(|c| c.spell_id),
            Some(MOUNT),
            "a held state stays"
        );
        assert!(sent_edges(&mut world).is_empty(), "and no precast plays");
    }

    /// The trade-slot click sends with its visuals hidden (`0x80c62c`): no state, no precast.
    /// Another cursor commit takes the state as a press does.
    #[test]
    fn the_trade_slot_send_hides_its_visuals() {
        use crate::creature_anim::Casting;
        let (mut world, _rx, me) = sender_world();
        let commit = |world: &mut World, bound: TargetedBind| {
            world.insert_resource(crate::spell::PendingCast::default());
            world
                .run_system_once(move |mut ladder: CastLadder| {
                    ladder.commit_targeted(MOUNT, CastCommit::Spell, bound);
                })
                .expect("the tail runs as a one-shot system");
            crate::ui_cast::PressEdges::run_tails(world);
        };
        commit(&mut world, TargetedBind::TradeItem);
        assert!(world.get::<Casting>(me).is_none());
        assert!(sent_edges(&mut world).is_empty());
        commit(&mut world, TargetedBind::Glyph(0));
        assert!(world.get::<Casting>(me).is_some_and(|c| c.hidden));
    }

    /// A flown aim marks an instant cast whose visual has a missile as a trajectory cast, which
    /// takes no state at the send (`0x80a09a`, `0x8053d6`); sent plain, or with no missile, it
    /// does.
    #[test]
    fn a_flown_instant_missile_aim_takes_no_state_at_the_send() {
        use crate::creature_anim::Casting;
        use std::collections::HashMap;
        const AIM: u32 = 900;
        const VISUAL: u32 = 77;
        let (mut world, _rx, me) = sender_world();
        let flown = |cast_index: u32| benilla_formats::SpellDisplay {
            visual: VISUAL,
            flies_aim: true,
            casting_time_index: cast_index,
            ..Default::default()
        };
        let mut visuals = HashMap::new();
        visuals.insert(
            VISUAL,
            benilla_formats::VisualStages {
                missile_gate: 1,
                ..Default::default()
            },
        );
        world.insert_resource(crate::creature_anim::SpellVisuals(
            benilla_formats::SpellVisualCatalog::from_tables(visuals, HashMap::new()),
        ));
        let mut spells = Spells::empty_for_tests();
        spells.catalog =
            benilla_formats::SpellCatalog::from_displays(HashMap::from([(AIM, flown(0))]));
        world.insert_resource(spells);
        let start = |world: &mut World, aimed: bool| {
            world.entity_mut(me).remove::<Casting>();
            world
                .run_system_once(move |mut ecs: Commands| {
                    start_sent_cast(&mut ecs, None, AIM, 1, aimed);
                })
                .expect("one-shot");
            world.get::<Casting>(me).is_some()
        };
        assert!(!start(&mut world, true), "a trajectory cast holds no state");
        assert!(start(&mut world, false), "sent plain, it does");
        let mut no_missile = world.resource_mut::<crate::creature_anim::SpellVisuals>();
        no_missile.0 = benilla_formats::SpellVisualCatalog::from_tables(
            HashMap::from([(VISUAL, Default::default())]),
            HashMap::new(),
        );
        assert!(start(&mut world, true), "no missile, no trajectory mark");
    }

    /// Every commit files its record on the pending list (`0x80cda8`), a ranged shot and an on-next-swing strike included, so the cast result's `modalNextSpell` test finds it.
    #[test]
    fn every_commit_files_its_record() {
        let (mut world, _rx) = combat_world(false);
        send_at(&mut world, SERPENT_STING, MOB);
        let pending = world.resource::<crate::spell::PendingCast>();
        assert_eq!(pending.committed(), Some(SERPENT_STING));
        let (mut world, _rx) = combat_world(false);
        send_at(&mut world, RAPTOR_STRIKE, MOB);
        let pending = world.resource::<crate::spell::PendingCast>();
        assert_eq!(pending.committed(), Some(RAPTOR_STRIKE));
    }

    /// The chained Auto Shot passes every rung after the sting: the sting's GCD is running, but
    /// Auto Shot's `StartRecoveryCategory` is 0 against the GCD node's 133.
    #[test]
    fn the_chained_auto_shot_is_not_refused_by_the_sting_it_followed() {
        let (mut world, rx) = combat_world(false);
        send_at(&mut world, SERPENT_STING, MOB);
        assert!(
            matches!(rx.try_recv(), Ok(ClientCommand::CastSpell { spell_id, .. }) if spell_id == SERPENT_STING),
            "the sting goes out first"
        );
        // What `cast_result` returns and `on_cast_result` hands to the ladder.
        send_at(&mut world, AUTO_SHOT, MOB);
        assert!(
            matches!(rx.try_recv(), Ok(ClientCommand::CastSpell { spell_id, .. }) if spell_id == AUTO_SHOT),
            "and the chained Auto Shot goes out behind it — no rung eats it"
        );
        assert_eq!(
            world.resource::<AutoRepeatActive>().0,
            Some(AUTO_SHOT),
            "...and the commit arms the repeat, which is the whole observable"
        );
    }

    /// A different spell pending sends (`0x80d34b` tests the pressed spell alone): no local `0x61`, and the pending cast keeps its record.
    #[test]
    fn a_shot_pressed_while_another_spell_is_pending_goes_out() {
        const FROSTBOLT: u32 = 116;
        let (mut world, rx) = combat_world(false);
        world
            .resource_mut::<crate::spell::PendingCast>()
            .arm(FROSTBOLT, 1);

        send_at(&mut world, AUTO_SHOT, MOB);

        assert!(
            rx.try_iter().any(
                |c| matches!(c, ClientCommand::CastSpell { spell_id, .. } if spell_id == AUTO_SHOT)
            ),
            "the shot goes out"
        );
        assert!(crate::ui_action::take_refusals(&mut world).is_empty());
        assert!(world
            .resource::<crate::spell::PendingCast>()
            .listed_for(FROSTBOLT, 0));
    }

    /// The press the attempt holds at its attack pick: its pick step waiting, or the harness's hold.
    fn held_at_pick(world: &mut World) -> Option<HeldCast> {
        let step = world
            .resource_mut::<crate::ui_cast::PressEdges>()
            .take_steps()
            .into_iter()
            .find_map(|e| match e {
                crate::ui_cast::CastBarEdge::AttackPick(rest) => rest.hold,
                _ => None,
            });
        step.or_else(|| world.resource_mut::<HeldForPick>().0.take())
    }

    /// [`world`] plus the catalog and the self player; `engaged` mirrors the attack lock
    /// `[+0xc48]`.
    fn combat_world(engaged: bool) -> (World, Receiver<ClientCommand>) {
        let (mut world, rx) = world();
        world.insert_resource(combat_catalog());
        let mut me = world.spawn(SelfPlayer);
        if engaged {
            me.insert(crate::creature_anim::Engaged(0));
        }
        (world, rx)
    }

    fn send_at(world: &mut World, spell_id: u32, target: u64) {
        world
            .run_system_once(move |mut ladder: CastLadder| {
                let ctx = cast_target::CastContext {
                    selection_guid: Some(target),
                    ..ctx()
                };
                ladder.send(spell_id, &ctx, CastCommit::Spell);
            })
            .expect("the ladder runs as a one-shot system");
        crate::ui_cast::PressEdges::run_tails(world);
    }

    /// The cast attempt's attack pick: a press of a spell stops there, above the bind, for its
    /// owner to pick a unit. Attacking skips the pick, and neither a spell without the bits nor a
    /// send that is not a press stops.
    #[test]
    fn a_combat_press_holds_at_the_attack_pick_unless_attacking() {
        let press = |world: &mut World, spell_id: u32| {
            world
                .run_system_once(move |mut ladder: CastLadder| {
                    ladder.send_spell(spell_id, &ctx(), None);
                })
                .expect("the ladder runs as a one-shot system");
            held_at_pick(world)
        };
        let (mut world, rx) = combat_world(false);
        let count = world.resource::<crate::spell::PendingCast>().next_count();
        assert_eq!(
            press(&mut world, RAPTOR_STRIKE),
            Some(HeldCast {
                spell_id: RAPTOR_STRIKE,
                passed: None,
                count,
            })
        );
        assert!(rx.try_recv().is_err(), "a held press sends nothing");
        assert!(crate::ui_action::take_refusals(&mut world).is_empty());
        assert_eq!(
            press(&mut world, SERPENT_STING),
            None,
            "a spell without the bits"
        );

        let (mut world, _rx) = combat_world(true);
        assert_eq!(press(&mut world, RAPTOR_STRIKE), None, "attacking");

        let (mut world, _rx) = combat_world(false);
        send(&mut world, RAPTOR_STRIKE, CastCommit::Spell);
        assert_eq!(world.resource::<HeldForPick>().0, None, "not a press");
    }

    /// The rungs above the pick refuse before it: a press mid-cast and a finisher with no combo
    /// points (`0x80d4e7`) hold nothing, so they never select a unit. A warrior reads
    /// `CASTER_AURASTATE` (0x12), any other class `NO_COMBO_POINTS` (0x46, `0x80d57d`).
    #[test]
    fn the_rungs_above_the_attack_pick_refuse_before_it() {
        use benilla_protocol::ObjectFields;
        // The same on-next-swing spell pending bails silently above the pick (`0x80d379`).
        let (mut world, rx) = combat_world(false);
        world
            .resource_mut::<crate::spell::PendingCast>()
            .arm(RAPTOR_STRIKE, 1);
        world
            .run_system_once(|mut ladder: CastLadder| {
                ladder.send_spell(RAPTOR_STRIKE, &ctx(), None);
            })
            .expect("the ladder runs as a one-shot system");
        assert_eq!(held_at_pick(&mut world), None, "pending");
        assert!(crate::ui_action::take_refusals(&mut world).is_empty());
        assert!(rx.try_recv().is_err());

        // `UNIT_FIELD_BYTES_0` byte 1 is the class; the context banks no points.
        for (class, reason) in [(4u32, 0x46), (11, 0x46), (1, 0x12)] {
            let (mut world, rx) = combat_world(false);
            world
                .run_system_once(move |mut ladder: CastLadder| {
                    let me = crate::net::ObjectStore(ObjectFields::from_pairs(&[(
                        benilla_protocol::field::FIELD_UNIT_BYTES_0,
                        class << 8,
                    )]));
                    let ctx = cast_target::CastContext {
                        rel: cast_target::TargetRelations {
                            self_store: Some(&me),
                            ..ctx().rel
                        },
                        ..ctx()
                    };
                    ladder.send_spell(EVISCERATE, &ctx, None);
                })
                .expect("the ladder runs as a one-shot system");
            assert_eq!(
                held_at_pick(&mut world),
                None,
                "class {class}: no combo points"
            );
            assert_eq!(
                crate::ui_action::take_refusals(&mut world),
                vec![CastFail::local(EVISCERATE, reason)],
                "class {class}"
            );
            assert!(rx.try_recv().is_err());
        }
    }

    /// The shot goes out first (`0x80d727`), then the repeat arm ends the swing (`0x80d99b` -> `0x800a5d`): `CMSG_ATTACKSTOP`, then the strike's cancel with its own count (`0x8062de`).
    #[test]
    fn an_auto_repeat_press_stops_the_attack_and_takes_the_queued_strike_with_it() {
        let (mut world, rx) = combat_world(true);
        world
            .resource_mut::<crate::spell::QueuedMeleeSpell>()
            .arm(RAPTOR_STRIKE, 7);

        send_at(&mut world, AUTO_SHOT, MOB);

        assert!(
            matches!(rx.try_recv(), Ok(ClientCommand::CastSpell { spell_id, .. }) if spell_id == AUTO_SHOT),
            "the shot commits first"
        );
        assert!(
            matches!(rx.try_recv(), Ok(ClientCommand::AttackStop)),
            "then the repeat arm's StopAttack"
        );
        assert!(
            matches!(rx.try_recv(), Ok(ClientCommand::CancelCast { spell_id, count: 7 }) if spell_id == RAPTOR_STRIKE),
            "the StopAttack tail cancels the queued strike with its record's count"
        );
        assert_eq!(
            world.resource::<crate::spell::QueuedMeleeSpell>().current(),
            None,
            "the queue is empty — the strike's checked ring goes dark"
        );
    }

    /// The repeat arm waits for the send's tail after `SENT`'s listeners (`0x80d727`, `0x80d99b`): the ladder leaves the repeat unset and `START_AUTOREPEAT_SPELL` unraised.
    #[test]
    fn the_repeat_arm_waits_for_the_sends_tail() {
        let (mut world, rx) = combat_world(false);
        world
            .run_system_once(|mut ladder: CastLadder| {
                let ctx = cast_target::CastContext {
                    selection_guid: Some(MOB),
                    ..ctx()
                };
                ladder.send(AUTO_SHOT, &ctx, CastCommit::Spell);
            })
            .expect("the ladder runs as a one-shot system");
        assert!(
            matches!(rx.try_recv(), Ok(ClientCommand::CastSpell { spell_id, .. }) if spell_id == AUTO_SHOT)
        );
        assert_eq!(
            world.resource::<AutoRepeatActive>().0,
            None,
            "unset at SENT"
        );
        let edges = world
            .resource::<crate::ui_cast::PressEdges>()
            .waiting()
            .to_vec();
        assert!(
            matches!(
                edges.first(),
                Some(crate::ui_cast::CastBarEdge::Sent { .. })
            ),
            "{edges:?}"
        );
        assert!(
            !edges.iter().any(|e| matches!(
                e,
                crate::ui_cast::CastBarEdge::Signal("START_AUTOREPEAT_SPELL")
            )),
            "{edges:?}"
        );
        crate::ui_cast::PressEdges::run_tails(&mut world);
        assert_eq!(world.resource::<AutoRepeatActive>().0, Some(AUTO_SHOT));
        let raised = world
            .resource::<crate::ui_script::live::UiEvents>()
            .cast_edges();
        assert!(
            raised.iter().any(|e| matches!(
                e,
                crate::ui_cast::CastBarEdge::Signal("START_AUTOREPEAT_SPELL")
            )),
            "{raised:?}"
        );
    }

    /// A cast over a wand repeat sends, then cancels the repeat (`0x80d9fa` -> `0x807560`), with no `CMSG_CANCEL_CAST` when no record of it is pending; Auto Shot survives.
    #[test]
    fn a_cast_over_a_wand_repeat_cancels_it_after_the_send() {
        let (mut world, rx) = combat_world(false);
        world.resource_mut::<AutoRepeatActive>().0 = Some(SHOOT);
        send_at(&mut world, SERPENT_STING, MOB);
        let sent: Vec<ClientCommand> = rx.try_iter().collect();
        assert!(
            matches!(sent.first(), Some(ClientCommand::CastSpell { spell_id, .. }) if *spell_id == SERPENT_STING),
            "{sent:?}"
        );
        assert!(
            matches!(sent.get(1), Some(ClientCommand::CancelAutoRepeat)),
            "{sent:?}"
        );
        assert!(
            !sent
                .iter()
                .any(|c| matches!(c, ClientCommand::CancelCast { .. })),
            "{sent:?}"
        );
        assert_eq!(world.resource::<AutoRepeatActive>().0, None);

        let (mut world, rx) = combat_world(false);
        world.resource_mut::<AutoRepeatActive>().0 = Some(AUTO_SHOT);
        send_at(&mut world, SERPENT_STING, MOB);
        assert!(!rx
            .try_iter()
            .any(|c| matches!(c, ClientCommand::CancelAutoRepeat)));
        assert_eq!(world.resource::<AutoRepeatActive>().0, Some(AUTO_SHOT));
    }

    /// The cast attempt's records take counts 1, 2, ... (`0x80cda8`, `0x8050dc`); a local cancel names the cancelled record's own count (`0x8062de`), not the spell's latest.
    #[test]
    fn a_cancel_names_its_records_own_count() {
        let (mut world, rx) = combat_world(false);
        send_at(&mut world, RAPTOR_STRIKE, MOB);
        let first = rx.try_iter().find_map(|c| match c {
            ClientCommand::CastSpell { count, .. } => Some(count),
            _ => None,
        });
        assert_eq!(first, Some(1));
        assert_eq!(
            world.resource::<crate::spell::QueuedMeleeSpell>().count(),
            Some(1)
        );
        send_at(&mut world, SERPENT_STING, MOB);
        let second = rx.try_iter().find_map(|c| match c {
            ClientCommand::CastSpell { count, .. } => Some(count),
            _ => None,
        });
        assert_eq!(second, Some(2));
        assert_eq!(
            world.resource::<crate::spell::QueuedMeleeSpell>().count(),
            Some(1),
            "the strike keeps its record's count"
        );
    }

    /// The reference's early-out (not attacking and `[+0xc50] == 0`): with no swing running nothing
    /// is sent and the queued strike survives.
    #[test]
    fn an_auto_repeat_press_with_no_swing_running_stops_nothing() {
        let (mut world, rx) = combat_world(false);
        world
            .resource_mut::<crate::spell::QueuedMeleeSpell>()
            .arm(RAPTOR_STRIKE, 1);

        send_at(&mut world, AUTO_SHOT, MOB);

        assert!(
            matches!(rx.try_recv(), Ok(ClientCommand::CastSpell { spell_id, .. }) if spell_id == AUTO_SHOT),
            "the shot, and only the shot"
        );
        assert!(rx.try_recv().is_err(), "no ATTACKSTOP, no CANCEL_CAST");
        assert_eq!(
            world.resource::<crate::spell::QueuedMeleeSpell>().current(),
            Some(RAPTOR_STRIKE)
        );
    }

    /// A strike pressed while engaged never reaches the attack start, so a running auto-repeat
    /// survives. Unreachable in play, since starting Auto Shot stops the swing.
    #[test]
    fn a_strike_pressed_while_already_swinging_reaches_no_attack_start_at_all() {
        let (mut world, rx) = combat_world(true);
        world.resource_mut::<AutoRepeatActive>().0 = Some(AUTO_SHOT);

        send_at(&mut world, RAPTOR_STRIKE, MOB);

        assert!(
            matches!(rx.try_recv(), Ok(ClientCommand::CastSpell { spell_id, .. }) if spell_id == RAPTOR_STRIKE)
        );
        assert!(
            rx.try_recv().is_err(),
            "no ATTACKSWING and no CANCEL_AUTO_REPEAT — skipped the whole attack-start"
        );
        assert_eq!(
            world.resource::<AutoRepeatActive>().0,
            Some(AUTO_SHOT),
            "the repeat survives: the reference cancels it via the attack it STARTS, and it started none"
        );
        assert_eq!(
            world.resource::<crate::spell::QueuedMeleeSpell>().current(),
            Some(RAPTOR_STRIKE),
            "the strike still queues — only the attack-start was skipped"
        );
    }

    /// Not yet swinging: the tail fires, the attack start runs, and it cancels the repeat.
    #[test]
    fn a_strike_pressed_before_the_swing_starts_kills_the_auto_repeat() {
        let (mut world, rx) = combat_world(false);
        world.resource_mut::<AutoRepeatActive>().0 = Some(AUTO_SHOT);

        send_at(&mut world, RAPTOR_STRIKE, MOB);

        assert!(
            matches!(rx.try_recv(), Ok(ClientCommand::CastSpell { spell_id, .. }) if spell_id == RAPTOR_STRIKE)
        );
        // The swing send, the repeat cancel after it.
        assert!(
            matches!(rx.try_recv(), Ok(ClientCommand::AttackSwing { guid }) if guid == MOB),
            "the swing goes out first"
        );
        assert!(
            matches!(rx.try_recv(), Ok(ClientCommand::CancelAutoRepeat)),
            "then the attack-start tail's"
        );
        assert_eq!(
            world.resource::<AutoRepeatActive>().0,
            None,
            "Auto Shot's ring goes dark"
        );
    }

    /// An item use raises `UNIT_SPELLCAST_SENT` first, on the send's shared tail (`0x80b2dc` -> `0x80b348`, `0x80b514`, `0x80b556`).
    #[test]
    fn an_item_use_raises_sent_ahead_of_its_tail() {
        let (mut world, _rx) = world();
        world
            .run_system_once(|mut ladder: CastLadder| {
                ladder.send(HEARTHSTONE, &ctx(), HEARTH_COMMIT);
            })
            .expect("the ladder runs as a one-shot system");
        let edges = world
            .resource::<crate::ui_cast::PressEdges>()
            .waiting()
            .to_vec();
        assert!(
            matches!(
                edges.first(),
                Some(crate::ui_cast::CastBarEdge::Sent {
                    spell_id: HEARTHSTONE,
                    ..
                })
            ),
            "{edges:?}"
        );
    }

    /// An unanswered item use keeps its record with no deadline (`0x805180`); its re-click with nothing cooling and a different spell go out, with no local `0x61`.
    #[test]
    fn an_unanswered_item_use_blocks_no_press() {
        let (mut world, rx) = world();
        let uses = |rx: &Receiver<ClientCommand>| {
            rx.try_iter()
                .filter(|c| matches!(c, ClientCommand::UseItem { .. }))
                .count()
        };
        send(&mut world, HEARTHSTONE, HEARTH_COMMIT);
        assert_eq!(uses(&rx), 1);
        let item = world.resource::<crate::spell::PendingCast>().records()[0];
        assert_eq!((item.caster, item.item), (HEARTH_GUID, Some(6948)));
        send(&mut world, HEARTHSTONE, HEARTH_COMMIT);
        assert_eq!(uses(&rx), 1, "the re-click goes out");
        send(&mut world, MOUNT, HEARTH_COMMIT);
        assert_eq!(uses(&rx), 1, "a different spell goes out");
        assert!(crate::ui_action::take_refusals(&mut world).is_empty());
    }

    /// The not-ready rung forks: an item refuses 0x28 "Item is not ready yet." (`609549`), a spell
    /// 0x3c (`609616`).
    #[test]
    fn the_not_ready_reason_forks_on_item_present() {
        let (mut world, rx) = world();
        let use_spell = benilla_protocol::messages::ItemUseSpell {
            spell_id: HEARTHSTONE,
            cooldown_ms: 1_800_000,
            category: 0,
            category_cooldown_ms: 0,
        };
        world.resource_mut::<crate::spell::Cooldowns>().start_item(
            6948,
            &use_spell,
            None,
            Instant::now(),
        );

        send(&mut world, HEARTHSTONE, HEARTH_COMMIT);
        assert!(rx.try_recv().is_err(), "an item on cooldown never sends");
        assert_eq!(
            crate::ui_action::take_refusals(&mut world),
            vec![CastFail::local(HEARTHSTONE, 0x28)]
        );

        // The record is keyed (use-spell, entry); a spell press queries (spell, 0) and passes.
        crate::ui_action::take_refusals(&mut world);
        world.insert_resource(crate::spell::PendingCast::default());
        send(&mut world, HEARTHSTONE, CastCommit::Spell);
        assert!(
            matches!(rx.try_recv(), Ok(ClientCommand::CastSpell { .. })),
            "the item-keyed record never not-readies a bare spell press"
        );
        assert!(crate::ui_action::take_refusals(&mut world).is_empty());
    }

    /// The cast send: two opcodes, one targets block; a key's lock rides it as
    /// `TARGET_FLAG_GAMEOBJECT|LOCKED`.
    #[test]
    fn the_commit_picks_the_opcode_and_the_block() {
        use benilla_protocol::messages::UseItemTarget;
        let (mut world, rx) = world();

        send(&mut world, HEARTHSTONE, CastCommit::Spell);
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::CastSpell {
                spell_id: HEARTHSTONE,
                target: None,
                ..
            })
        ));

        // Reset the in-flight guard the previous send armed.
        world.insert_resource(crate::spell::PendingCast::default());
        send(&mut world, HEARTHSTONE, HEARTH_COMMIT);
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::UseItem {
                bag_index: 255,
                slot: 24,
                spell_id: HEARTHSTONE,
                item_guid: HEARTH_GUID,
                target: UseItemTarget::SelfImplicit,
                ..
            })
        ));

        world.insert_resource(crate::spell::PendingCast::default());
        send(
            &mut world,
            HEARTHSTONE,
            CastCommit::Item {
                bag_index: 255,
                slot: 81,
                entry: 6948,
                item_guid: HEARTH_GUID,
                on_object: Some(0xF110_000C_1F00_A3B2),
            },
        );
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::UseItem {
                slot: 81,
                target: UseItemTarget::Object(0xF110_000C_1F00_A3B2),
                ..
            })
        ));
    }

    /// The power gate: an unaffordable spell refuses 0x4d locally; an item press skips it.
    #[test]
    fn an_unaffordable_press_refuses_locally_and_items_skip_the_gate() {
        use crate::net::ObjectStore;
        use benilla_formats::SpellDisplay;
        use benilla_protocol::ObjectFields;

        let (mut world, rx) = world();
        let mut displays = std::collections::HashMap::new();
        displays.insert(
            HEARTHSTONE,
            SpellDisplay {
                power_type: 0,
                mana_cost: 500,
                ..Default::default()
            },
        );
        let mut spells = crate::ui_action::Spells::empty_for_tests();
        spells.catalog = benilla_formats::SpellCatalog::from_displays(displays);
        world.insert_resource(spells);
        // 100 mana (field 23 = UNIT_FIELD_POWER1). Leaked: the one-shot closure must be 'static.
        let store: &'static ObjectStore =
            Box::leak(Box::new(ObjectStore(ObjectFields::from_pairs(&[
                (benilla_protocol::field::FIELD_UNIT_HEALTH, 100u32),
                (benilla_protocol::field::FIELD_UNIT_POWER1, 100),
            ]))));

        let base = ctx();
        let with_store = cast_target::CastContext {
            rel: cast_target::TargetRelations {
                self_store: Some(store),
                ..base.rel
            },
            ..base
        };
        world
            .run_system_once(move |mut ladder: CastLadder| {
                ladder.send(HEARTHSTONE, &with_store, CastCommit::Spell);
            })
            .expect("one-shot");
        assert!(rx.try_recv().is_err(), "an unaffordable press never sends");
        assert_eq!(
            crate::ui_action::take_refusals(&mut world),
            vec![CastFail::local(HEARTHSTONE, 0x4d)]
        );

        crate::ui_action::take_refusals(&mut world);
        let base = ctx();
        let with_store = cast_target::CastContext {
            rel: cast_target::TargetRelations {
                self_store: Some(store),
                ..base.rel
            },
            ..base
        };
        world
            .run_system_once(move |mut ladder: CastLadder| {
                ladder.send(HEARTHSTONE, &with_store, HEARTH_COMMIT);
            })
            .expect("one-shot");
        assert!(
            matches!(rx.try_recv(), Ok(ClientCommand::UseItem { .. })),
            "an item press is never power-gated"
        );
        assert!(crate::ui_action::take_refusals(&mut world).is_empty());
    }

    /// The targeting commit over its commit × bind grid; every cell arms the guard and clears the
    /// word.
    #[test]
    fn the_targeting_commit_tail_covers_every_seam() {
        use benilla_protocol::messages::UseItemTarget;
        const DEST: benilla_protocol::messages::TargetPoint =
            benilla_protocol::messages::TargetPoint {
                transport: 0,
                pos: [1.0, 2.0, 3.0],
            };
        const ITEM: u64 = 0xF150_0000_0000_ABCD;
        const GO: u64 = 0xF110_000C_1F00_A3B2;
        let (mut world, rx) = world();
        let commit = |world: &mut World, commit: CastCommit, bound: TargetedBind| {
            world.insert_resource(crate::spell::PendingCast::default());
            world
                .resource_mut::<crate::spell::SpellTargeting>()
                // The lock word, which answers both the bag and the world click.
                .enter(HEARTHSTONE, commit, 0x4800);
            world
                .run_system_once(move |mut ladder: CastLadder| {
                    ladder.commit_targeted(HEARTHSTONE, commit, bound);
                })
                .expect("the tail runs as a one-shot system");
        };

        commit(&mut world, CastCommit::Spell, TargetedBind::Dest(DEST));
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::CastSpellAtDest { dest: DEST, .. })
        ));
        assert!(
            !world.resource::<crate::spell::SpellTargeting>().active(),
            "the commit clears the one word"
        );
        assert!(
            world.resource::<crate::spell::PendingCast>().in_flight(),
            "and arms the in-flight guard the click owes"
        );

        commit(&mut world, CastCommit::Spell, TargetedBind::Item(ITEM));
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::CastSpellItem {
                item_guid: ITEM,
                ..
            })
        ));

        commit(&mut world, HEARTH_COMMIT, TargetedBind::Dest(DEST));
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::UseItem {
                bag_index: 255,
                slot: 24,
                target: UseItemTarget::Dest(DEST),
                ..
            })
        ));

        commit(&mut world, HEARTH_COMMIT, TargetedBind::Item(ITEM));
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::UseItem {
                bag_index: 255,
                slot: 24,
                target: UseItemTarget::Item(ITEM),
                ..
            })
        ));

        // The world click: the GameObject block, the same builder as the right-click OPEN_LOCK.
        commit(&mut world, CastCommit::Spell, TargetedBind::Object(GO));
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::CastSpellGameObject { go_guid: GO, .. })
        ));

        commit(&mut world, HEARTH_COMMIT, TargetedBind::Object(GO));
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::UseItem {
                bag_index: 255,
                slot: 24,
                target: UseItemTarget::Object(GO),
                ..
            })
        ));
    }

    /// Renew's ranks 2 and 3 (`Spell.dbc`): spell levels 14 and 20, aura 8, implicit target 21.
    const RENEW_2: u32 = 6074;
    const RENEW_3: u32 = 6075;
    const ALLY: u64 = 0x11;

    /// A press of Renew rank 3 by a level-20 priest at `ally_level`, the ally a player of our own
    /// faction: what went out, and what the in-flight guard holds for the go to clear.
    fn renew_at(ally_level: u32, at_self: bool) -> (Option<ClientCommand>, Option<u32>) {
        use benilla_formats::{FactionCatalog, FactionTemplate, SpellCatalog, SpellDisplay};
        use benilla_protocol::field::{
            FIELD_OBJECT_TYPE, FIELD_UNIT_FACTIONTEMPLATE, FIELD_UNIT_FLAGS, FIELD_UNIT_HEALTH,
            FIELD_UNIT_LEVEL,
        };
        use std::collections::HashMap;
        let (mut world, rx) = world();
        let renew = |id, spell_level| SpellDisplay {
            id,
            name: "Renew".into(),
            spell_level,
            implicit_target_a1: 21,
            effect_apply_aura: [8, 0, 0],
            ..Default::default()
        };
        world.insert_resource(Spells {
            catalog: SpellCatalog::from_displays(HashMap::from([
                (RENEW_2, renew(RENEW_2, 14)),
                (RENEW_3, renew(RENEW_3, 20)),
            ])),
            ..Spells::empty_for_tests()
        });
        world.insert_resource(crate::ui_action::PlayerActions {
            spells: [RENEW_2, RENEW_3].into(),
            ..Default::default()
        });
        world.spawn(SelfPlayer);
        // A live, player-controlled player of faction template 1, which assists its own kind.
        let player = |level: u32| {
            crate::net::ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                (FIELD_OBJECT_TYPE, 0x19),
                (FIELD_UNIT_FACTIONTEMPLATE, 1),
                (FIELD_UNIT_FLAGS, 0x8),
                (FIELD_UNIT_HEALTH, 100),
                (FIELD_UNIT_LEVEL, level),
            ]))
        };
        let (me, ally) = (player(20), player(ally_level));
        let factions = crate::target::Factions::from_catalog(FactionCatalog::from_rows(
            HashMap::from([(
                1,
                FactionTemplate {
                    faction: 1,
                    group_mask: 3,
                    friend_group_mask: 2,
                    enemy_group_mask: 12,
                    enemies: [0; 4],
                    friends: [0; 4],
                },
            )]),
            HashMap::new(),
        ));
        world
            .run_system_once(move |mut ladder: CastLadder| {
                let base = ctx();
                let ctx = cast_target::CastContext {
                    selection_guid: Some(if at_self { 0x7 } else { ALLY }),
                    rel: cast_target::TargetRelations {
                        target_store: Some(if at_self { &me } else { &ally }),
                        self_store: Some(&me),
                        factions: Some(&factions),
                        ..base.rel
                    },
                    ..base
                };
                ladder.send(RENEW_3, &ctx, CastCommit::Spell);
            })
            .expect("the ladder runs as a one-shot system");
        let sent = rx
            .try_iter()
            .find(|c| matches!(c, ClientCommand::CastSpell { .. }));
        let armed = world.resource::<crate::spell::PendingCast>().committed();
        (sent, armed)
    }

    /// The reported bug: Renew on an ally more than ten levels below it went out at the pressed
    /// rank while the server cast the rank the ally takes, so the go never matched the guard and
    /// every press for five seconds read "Another action is in progress". The send's rank pick
    /// (`0x53d9d0`) now sends and guards the rank the ally takes; within ten levels, and on
    /// ourselves, the pressed rank stands.
    #[test]
    fn a_buff_on_a_low_ally_sends_and_guards_the_rank_it_takes() {
        let (sent, armed) = renew_at(9, false);
        assert!(
            matches!(
                sent,
                Some(ClientCommand::CastSpell {
                    spell_id: RENEW_2,
                    target: Some(ALLY),
                    ..
                })
            ),
            "rank 2 goes out at the ally: {sent:?}"
        );
        assert_eq!(
            armed,
            Some(RENEW_2),
            "the guard names the rank the go will name"
        );

        let (sent, armed) = renew_at(10, false);
        assert!(
            matches!(
                sent,
                Some(ClientCommand::CastSpell {
                    spell_id: RENEW_3,
                    ..
                })
            ),
            "within ten levels: {sent:?}"
        );
        assert_eq!(armed, Some(RENEW_3));

        let (sent, _) = renew_at(9, true);
        assert!(
            matches!(
                sent,
                Some(ClientCommand::CastSpell {
                    spell_id: RENEW_3,
                    ..
                })
            ),
            "ourselves at our own level: {sent:?}"
        );
    }
}
