//! The action-bar drains: a queued `UseAction` becomes a cast, a swing, an item use or a macro
//! run; a queued `PickupAction`/`PlaceAction` becomes one `CMSG_SET_ACTION_BUTTON`, which gets no
//! reply, so a drag-swap is two independent sends.

use bevy::prelude::*;

use benilla_protocol::messages::{ACTION_KIND_ITEM, ACTION_KIND_MACRO, ACTION_KIND_SPELL};
use benilla_ui::script::UiScript;

use crate::spell::{cast_target, CastLadder};

use super::{attack_actor_refusal, ActionSlots, SPELL_ATTACK};

/// What clicking an item action does, and to which copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ItemRoute {
    /// Use this copy: the wire `(bag_index, slot)` and its instance guid.
    Use((u8, u8, u64)),
    Equip((u8, u8, u64)),
    /// No copy found: the click does nothing.
    Nowhere,
}

/// The reference's equip-or-use decision for an item action: `InventoryType` 0 always uses;
/// otherwise a worn copy (equipment slots 0..18) uses in place and a carried one equips. `find` is
/// the inventory walk with the entry already bound.
pub(super) fn item_action_route(
    template: &benilla_protocol::ItemInfo,
    find: impl Fn(crate::ui_items::ItemSearch) -> Option<(u8, u8, u64)>,
) -> ItemRoute {
    let anywhere = |live_charges_only| crate::ui_items::ItemSearch {
        equipment_only: false,
        live_charges_only,
    };
    if template.inventory_type == 0 {
        // The use leg's charge filter: spent copies are skipped for finite charges.
        return match find(anywhere(template.has_finite_charges())) {
            Some(pos) => ItemRoute::Use(pos),
            None => ItemRoute::Nowhere,
        };
    }
    if let Some(pos) = find(crate::ui_items::ItemSearch {
        equipment_only: true,
        live_charges_only: false,
    }) {
        return ItemRoute::Use(pos);
    }
    match find(anywhere(false)) {
        Some(pos) => ItemRoute::Equip(pos),
        None => ItemRoute::Nowhere,
    }
}

/// The melee probe's press with nothing selected, the player's Attack
/// ([`AttackPress::attack_target`]), as the ATTACKTARGET binding's `AttackTarget()` is.
pub(crate) fn attack_nearest_probe(
    mut probe: MessageReader<crate::target::AttackNearestRequest>,
    mut press: AttackPress,
    mut signals: crate::ui_action::signals::QueuedSignals,
) {
    if probe.read().last().is_some() && press.selection.guid.is_none() {
        press.attack_target(&mut signals);
        signals.flush(&mut press.seam.ecs);
    }
}

/// What the player's Attack reads and writes: `AttackTarget`, the ATTACKTARGET binding and
/// `UseAction`'s Attack slot (through the cast attempt) all run this one press.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct AttackPress<'w, 's> {
    pick: crate::target::AttackPick<'w, 's>,
    selection: ResMut<'w, crate::target::Selection>,
    seam: crate::creature_anim::AttackSeam<'w, 's>,
    player: Option<Res<'w, crate::player::Player>>,
    auto_dismount: Option<Res<'w, crate::spell::cast_target::AutoDismount>>,
}

impl AttackPress<'_, '_> {
    /// The attack validator runs first, its actor checks then its target pick, which may move the
    /// selection; a refusal ends the press. Then the toggle: attacking stops, else it starts at the
    /// validator's target. Only the start cancels auto-repeat, so stopping melee leaves Auto Shot
    /// running.
    pub(crate) fn attack_target(&mut self, sink: &mut dyn crate::ui_action::signals::SignalSink) {
        let Some(guid) = self.validate(None, true, sink) else {
            return;
        };
        let Self { pick, seam, .. } = self;
        let Ok(e) = seam.me.single() else {
            return;
        };
        let engaged = pick.engaged();
        if let (false, Some(own)) = (engaged, pick.player().0) {
            crate::creature_anim::dismount_first(own, &seam.net);
        }
        debug!(
            "attack {} at {guid:#x}",
            if engaged { "toggled off" } else { "swing" }
        );
        crate::creature_anim::toggle_attack_local(
            e,
            guid,
            engaged,
            seam.queued_melee.as_mut(),
            seam.auto_repeat.as_mut(),
            &mut seam.sheath,
            &mut seam.ecs,
            &seam.net,
            sink,
        );
    }

    /// Whether the actor checks pass and we are not already attacking, `StartAttack`'s named leg
    /// running before its selection moves.
    pub(crate) fn may_start(
        &mut self,
        sink: &mut dyn crate::ui_action::signals::SignalSink,
    ) -> bool {
        if self.pick.engaged() {
            return false;
        }
        let mounted = self.mounted_gate();
        let (me, my_guid) = self.pick.player();
        !attack_actor_refusal(me, my_guid, mounted, sink)
    }

    fn mounted_gate(&self) -> crate::spell::validator::MountedGate {
        crate::spell::validator::MountedGate::player(
            self.player.as_deref().map_or(0, |p| p.move_flags()),
            self.auto_dismount.as_deref(),
        )
    }

    /// `StartAttack` named nobody: the refusal, with the selection left alone (`0x72c2b0`).
    pub(crate) fn no_attack_target(sink: &mut dyn crate::ui_action::signals::SignalSink) {
        sink.line(crate::ui_action::UiError::key("ERR_NO_ATTACK_TARGET"));
    }

    /// `StartAttack`'s swing at the selection a named unit left, past the nearest-enemy pick
    /// (`0x72c2b0`).
    pub(crate) fn swing_at_selection(
        &mut self,
        sink: &mut dyn crate::ui_action::signals::SignalSink,
    ) {
        let Self {
            pick,
            selection,
            seam,
            ..
        } = self;
        let gated = pick.gated(selection.as_ref(), true, sink);
        let Some(guid) = gated else {
            return;
        };
        let Ok(e) = seam.me.single() else {
            return;
        };
        if let Some(own) = pick.player().0 {
            crate::creature_anim::dismount_first(own, &seam.net);
        }
        crate::creature_anim::toggle_attack_local(
            e,
            guid,
            false,
            seam.queued_melee.as_mut(),
            seam.auto_repeat.as_mut(),
            &mut seam.sheath,
            &mut seam.ecs,
            &seam.net,
            sink,
        );
    }

    /// `StartAttack()` with no unit: the press, unless already attacking.
    pub(crate) fn start_attack(&mut self, sink: &mut dyn crate::ui_action::signals::SignalSink) {
        if !self.pick.engaged() {
            self.attack_target(sink);
        }
    }

    /// `StopAttack()` (`0x51d0b0`): only while attacking.
    pub(crate) fn stop_attack(&mut self, sink: &mut dyn crate::ui_action::signals::SignalSink) {
        if self.pick.engaged() {
            self.attack_target_off(sink);
        }
    }

    fn attack_target_off(&mut self, sink: &mut dyn crate::ui_action::signals::SignalSink) {
        let Self { seam, .. } = self;
        let Ok(e) = seam.me.single() else {
            return;
        };
        crate::creature_anim::toggle_attack_local(
            e,
            0,
            true,
            seam.queued_melee.as_mut(),
            seam.auto_repeat.as_mut(),
            &mut seam.sheath,
            &mut seam.ecs,
            &seam.net,
            sink,
        );
    }

    /// The attempt's attack pick for a held spell press (`0x80d661`): the validator alone, returning the unit it bound into the attempt (`0x80d65e`) or `None` when the attempt ends.
    pub(crate) fn pick_for_cast(
        &mut self,
        held: crate::spell::HeldCast,
        sink: &mut dyn crate::ui_action::signals::SignalSink,
    ) -> Option<u64> {
        self.validate(held.passed, false, sink)
    }

    /// The attack validator: its actor checks, then its target pick. `check_range` is on for the
    /// Attack action (`0x72c2de`) and off for the cast attempt (`0x80d661`).
    fn validate(
        &mut self,
        passed: Option<u64>,
        check_range: bool,
        sink: &mut dyn crate::ui_action::signals::SignalSink,
    ) -> Option<u64> {
        let mounted = self.mounted_gate();
        let Self {
            pick,
            selection,
            seam,
            ..
        } = self;
        let (me, my_guid) = pick.player();
        if attack_actor_refusal(me, my_guid, mounted, sink) {
            return None;
        }
        pick.target(passed, selection.as_mut(), seam, check_range, sink)
    }
}

/// Sends the queued GameObject openers through the cast ladder. The reference reaches its cast
/// attempt from a right-click as from a button, so every rung applies: a second click on a chest
/// mid-cast is the ladder's silent same-spell bail.
pub(super) fn drain_go_openers(world: &mut World) {
    let openers = std::mem::take(&mut world.resource_mut::<crate::ui_action::GoOpenerCasts>().0);
    // Each opener is its own press, its raises fired before the next.
    for opener in openers {
        crate::script_calls::press_step(world, go_opener, opener);
    }
}

/// One GameObject opener.
fn go_opener(
    In(opener): In<super::GoOpener>,
    script: Option<NonSendMut<benilla_ui::script::UiScript>>,
    mut ui: ResMut<crate::ui_script::live::UiEvents>,
    targeting: cast_target::CastTargeting,
    mut ladder: CastLadder,
    mut gate: crate::ui_bind_confirm::BindGate,
) {
    let ctx = targeting.context();
    match opener {
        super::GoOpener::Spell { spell_id, go_guid } => {
            debug!("ui_action: gameobject opener casts {spell_id} at {go_guid:#x}");
            ladder.send_at_object(spell_id, &ctx, go_guid);
        }
        // The key goes through the shared item-use fork, as a bag click does.
        super::GoOpener::Key(it) => {
            let Some(mut script) = script else {
                return;
            };
            debug!(
                "ui_action: gameobject opener uses the key at wire {}/{} on {:?}",
                it.bag_index, it.slot, it.on_object
            );
            crate::ui_items::send_item_use(it, &ctx, &mut ladder, &mut script, &mut gate, false);
        }
    }
    // The press's cast-bar edges, fired at its step (`0x80b556`).
    ladder.press.defer(&mut ui);
}

/// What a `UseAction` press reads and writes.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct ActionPress<'w, 's> {
    actions: Option<Res<'w, ActionSlots>>,
    targeting: cast_target::CastTargeting<'w, 's>,
    ladder: CastLadder<'w, 's>,
    gate: crate::ui_bind_confirm::BindGate<'w>,
    /// Resolves the press's unit token where it takes effect.
    tokens: crate::ui_unit::UnitTokens<'w, 's>,
}

/// What a `UseAction` press leaves to another applier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use]
pub(crate) enum UseOutcome {
    Done,
    /// The Attack slot passed the cast attempt's dead rung: the player's Attack ([`AttackPress`])
    /// runs.
    Attack,
    /// A pet-action slot: the pet bar's action at this 0-based index is pressed.
    PetAction(u32),
    /// A macro slot: the 1-based macro runs, its lines' calls applying inside it (`0x5abdbc`).
    Macro(u32),
}

/// `UseAction` on the pressed slot: a cast, an item use or a macro run, at the selection as the
/// calls before it left it, or the Attack slot, which it hands back for the player's Attack to run.
/// A macro slot comes back for the caller to run once the press's borrows end.
pub(crate) fn use_action(
    p: &mut ActionPress,
    script: &mut UiScript,
    press: benilla_ui::script::ActionUse,
) -> UseOutcome {
    let ActionPress {
        actions,
        targeting,
        ladder,
        gate,
        tokens,
    } = p;
    let selection = targeting.selection();
    let action = press.action;
    let slot = match u8::try_from(action.saturating_sub(1)) {
        Ok(s) => s,
        Err(_) => return UseOutcome::Done,
    };
    // Copied out: a macro's verbs may write the bar inside the press.
    let button = actions
        .as_deref()
        .and_then(|a| a.buttons.get(&slot).copied());
    match button.as_ref() {
        Some(b) if b.kind == ACTION_KIND_SPELL && b.action == SPELL_ATTACK => {
            // `UseAction` casts Attack through the cast attempt, so its dead rung comes first: "You
            // are dead", where the binding, which skips the attempt, reads "Can't attack while
            // dead." Then the attempt's effect-0x4e short-circuit runs the Attack toggle.
            if ladder.dead_refusal(SPELL_ATTACK, targeting.self_store.iter().next()) {
                return UseOutcome::Done;
            }
            return UseOutcome::Attack;
        }
        Some(b) if b.kind == ACTION_KIND_SPELL => {
            // In `UseAction` only: re-pressing the spell whose targeting cursor is up cancels it
            // before the cast attempt. A spellbook re-press aborts and re-enters.
            if ladder.ground.spell() == Some(b.action) {
                debug!(
                    "ui_action: cast {} re-pressed — targeting toggles off",
                    b.action
                );
                let item = ladder.ground.cast_item();
                ladder.press.cast_item_cancelled(item);
                ladder.ground.clear();
                return UseOutcome::Done;
            }
            // `ClickSpell` (`0x80da80`): an active spell re-pressed cancels. The form-match toggle
            // is `CastSpell`'s alone, not `UseAction`'s.
            let auras = targeting.self_auras.iter().next();
            let store = targeting.self_store.iter().next();
            if ladder.is_active_spell(b.action, store, auras) {
                if ladder.keeps_fresh_toggle(b.action) {
                    debug!("ui_action: cast {} re-pressed — just cast, kept", b.action);
                    return UseOutcome::Done;
                }
                debug!("ui_action: cast {} re-pressed — cancels", b.action);
                ladder.cancel_active_spell(b.action, store, auras);
                return UseOutcome::Done;
            }
            debug!(
                "ui_action: cast {} at {:?} (selection {:?})",
                b.action, press.at, selection.guid
            );
            let aim = targeting.aim(tokens, &press.at, crate::ui_unit::VmUnits::of(script));
            ladder.send_spell(b.action, &targeting.context_at(aim), aim.passed());
        }
        // An item action names an entry, so the click finds a copy. A miss only logs, with no
        // red error line: nothing was attempted.
        Some(b) if b.kind == ACTION_KIND_ITEM => {
            let Some(store) = targeting.self_store.iter().next() else {
                return UseOutcome::Done;
            };
            let template = ladder
                .items
                .template(b.action, 0, &ladder.commands)
                .cloned();
            // The reference bails on a null template too; the icon resolve usually cached it.
            let Some(template) = template else {
                debug!(
                    "ui_action: item action {action} (entry {}) has no template yet — skipped",
                    b.action
                );
                return UseOutcome::Done;
            };
            let route = item_action_route(&template, |s| {
                crate::ui_items::find_item(&store.0, &ladder.objects, b.action, s)
            });
            let ((bag_index, slot0, guid), equip) = match route {
                ItemRoute::Use(pos) => (pos, false),
                ItemRoute::Equip(pos) => (pos, true),
                ItemRoute::Nowhere => {
                    debug!(
                        "ui_action: item action {action} (entry {}) is nowhere in the inventory — skipped",
                        b.action
                    );
                    return UseOutcome::Done;
                }
            };
            if equip {
                // No quest guard: the bar tests only `InventoryType`, unlike the bag click
                // (which starts the quest), so a quest-starter equips.
                debug!("ui_action: item action {action} auto-equip (wire {bag_index}/{slot0})");
                // The shared auto-equip sender, so a BoE asks before binding, as from a bag.
                crate::ui_items::send_auto_equip(
                    script,
                    gate,
                    &ladder.objects,
                    &ladder.items,
                    &ladder.commands,
                    bag_index,
                    slot0,
                    Some(guid),
                    false,
                );
            } else {
                // The shared item-use fork: a quest-starter offers its quest, and the use runs the
                // whole cast ladder.
                debug!("ui_action: item action {action} use (wire {bag_index}/{slot0})");
                crate::ui_items::send_item_use(
                    crate::ui_items::ItemUse {
                        guid: Some(guid),
                        start_quest: template.start_quest,
                        bag_index,
                        slot: slot0,
                        entry: b.action,
                        use_spell: template.use_spell.map(|u| u.spell_id),
                        on_object: None,
                        is_charter: template.flags & benilla_protocol::messages::ITEM_FLAG_CHARTER
                            != 0,
                    },
                    &targeting.context(),
                    ladder,
                    script,
                    gate,
                    false,
                );
            }
        }
        // The macro arm (`0x5abda2`): the macro runs once the press's borrows end.
        Some(b) if b.kind == ACTION_KIND_MACRO => return UseOutcome::Macro(b.action),
        // A pet-action slot presses the pet bar's action by index, at no unit (`0x5ac000` →
        // `0x5d4210`).
        Some(b) if b.kind == super::ACTION_KIND_PET => return UseOutcome::PetAction(b.action),
        Some(b) => {
            debug!(
                "ui_action: action {action} kind {:#04x} has no use path",
                b.kind
            );
        }
        None => debug!("ui_action: UseAction({action}) on an empty slot"),
    }
    UseOutcome::Done
}
