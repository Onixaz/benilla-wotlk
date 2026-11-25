//! Where the script calls that touch the selection, the player's cast or the targeting cursor
//! take effect. In the reference each such call is done when it returns (the macro runner fires
//! every line in one pass; `TargetByName` and `TargetUnit` commit through the selection setter
//! `0x524bf0`; `CastSpellByName` casts at the selection as it stands then), so `/target Bob` then
//! `/cast Flash Heal` heals Bob, and `/cast` then `/target` casts at the old target. A system that
//! runs Lua with the `World` lent ([`crate::ui_script::safe_world`]) has each call applied inside
//! the Lua call that made it ([`apply_in_call`]), each through its owner's applier;
//! [`apply_script_calls`] applies what waited where no `World` was lent.

use bevy::prelude::*;

use benilla_ui::script::{ScriptCall, UiScript};

/// What applying one call left to do once its appliers' borrows ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Applied {
    /// The call is done.
    Done,
    /// `UseAction` on a macro slot: the 1-based macro, whose lines run inside the call (`0x5abdbc`).
    Macro(u32),
}

/// Apply `call` inside the Lua call that made it, with the lent `World`; `false` in a world
/// without the appliers' state, where the call waits. The applier raises nothing while it holds
/// the `World`: its signals, lines and the press's edges queue, and fire as the call returns with
/// the `World` lent again, so a listener's own call applies nested inside the raise.
pub(crate) fn apply_in_call(lua: &mlua::Lua, call: ScriptCall) -> bool {
    use crate::ui_script::safe_world::with_world;
    // The destroy in its turn, through the lent view (`0x5197d0`), then its signal.
    if let ScriptCall::DestroyTotem(slot) = call {
        if with_world(|_| ()).is_none() {
            return false;
        }
        let slot = usize::from(slot);
        if benilla_ui::script::write_live(|l| l.destroy_totem(slot)) == Some(true) {
            UiScript::in_call(lua).fire_event(
                "PLAYER_TOTEM_UPDATE",
                vec![benilla_ui::script::ScriptValue::Int(slot as i64 + 1)],
            );
        }
        return true;
    }
    // A loot verb acts and raises inside the call (`0x589140`, `0x589300`, `0x5893a0`, `0x523640`).
    if let ScriptCall::Loot(verb) = call {
        return with_world(|world| in_call_step(world, |w| crate::ui_loot::run_verb(w, verb)))
            .is_some();
    }
    // The verbs that press off a queued intent: their drain, each press at its own step.
    if let ScriptCall::Press(verb) = call {
        return with_world(|world| press_verb(world, verb)).is_some();
    }
    let applied = with_world(|world| {
        // The setter's tail at the selection write (`0x524f38`-`0x524f54`), ahead of what the call
        // raises after it: the cast's attack pick selects before the send (`0x80d661`, `0x80b556`).
        step_then_fire(
            world,
            |world| world.run_system_cached_with(apply_one, call).ok(),
            |world, applied| {
                if applied.is_some() {
                    crate::ui_unit::target_set_tail(world);
                }
            },
        )
    })
    .flatten();
    match applied {
        None => false,
        Some(Applied::Done) => true,
        // The macro's lines run through the chat input, as if typed, each line's call applying
        // as the line runs (`0x564db0`, `0x564e5d`).
        Some(Applied::Macro(index)) => {
            if !crate::ui_macro::run_macro(&mut UiScript::in_call(lua), index) {
                debug!("ui_action: macro {index} is empty");
            }
            true
        }
    }
}

/// Run `step`, a host step that holds the lent `World`, inside the Lua call that reached it: it
/// raises nothing while it holds the `World`; its signals, lines and a press's edges queue, and
/// fire as it returns with the `World` lent again, so a listener's own call applies nested.
pub(crate) fn in_call_step<R>(world: &mut World, step: impl FnOnce(&mut World) -> R) -> R {
    step_then_fire(world, step, |_, _| ())
}

/// [`in_call_step`], with `tail` run on the step's result after its borrows end and ahead of its
/// raises.
fn step_then_fire<R>(
    world: &mut World,
    step: impl FnOnce(&mut World) -> R,
    tail: impl FnOnce(&mut World, &R),
) -> R {
    use crate::ui_script::live::UiEvents;
    let Some(lua) = world
        .get_non_send_resource::<UiScript>()
        .map(|s| s.lua().clone())
    else {
        let out = step(world);
        tail(world, &out);
        return out;
    };
    // What an enclosing turn queued stays its own, fired by it.
    let earlier = world.get_resource_or_init::<UiEvents>().set_aside();
    let (out, raised) = benilla_ui::script::world_loan::raises_in_turn(&lua, || step(world));
    tail(world, &out);
    // The step's raises with the `World` lent again, each last on its path or followed by its rest as a step (`0x5d8ad3`, `0x706feb`, `0x5216f0`, `0x80b556`, `0x806200`).
    if !raised.is_empty() {
        crate::ui_script::safe_world::lend(world, |script| {
            for raise in raised {
                raise.run(script);
            }
        });
    }
    crate::ui_script::live::fire_handler_events(world);
    world.resource_mut::<UiEvents>().put_back(earlier);
    out
}

/// A verb's press inside its call: the drain of the intents it queued, each press at its own step ([`press_step`]), so the line after the call reads what it armed.
fn press_verb(world: &mut World, verb: benilla_ui::script::VerbPress) {
    use benilla_ui::script::VerbPress;
    match verb {
        VerbPress::TradeSkill => crate::ui_tradeskill::press_trade_skill_casts(world),
        VerbPress::DropItemOnUnit => crate::ui_action::drop_item::press_dropped_tokens(world),
        VerbPress::Death => crate::death::drain_death(world),
        VerbPress::ItemPicks => crate::spell::targeting::commit_item_cast_on_pick(world),
    }
}

/// One press of an exclusive holder at its own step: its raises and edges fire with the `World` lent before the holder's next press, which reads what it armed (`0x80b556`, `0x80b583`); earlier queues stay set aside.
pub(crate) fn press_step<I, O, M, S>(world: &mut World, system: S, input: I::Inner<'_>) -> Option<O>
where
    I: bevy::ecs::system::SystemInput + 'static,
    O: 'static,
    S: IntoSystem<I, O, M> + 'static,
{
    use crate::ui_script::live::UiEvents;
    let lua = world
        .get_non_send_resource::<UiScript>()
        .map(|s| s.lua().clone());
    let earlier = world.get_resource_or_init::<UiEvents>().set_aside();
    let (out, raised) = match &lua {
        Some(lua) => benilla_ui::script::world_loan::raises_in_turn(lua, || {
            step_ran(world.run_system_cached_with(system, input))
        }),
        None => (
            step_ran(world.run_system_cached_with(system, input)),
            Vec::new(),
        ),
    };
    assert_one_raise_list(world, !raised.is_empty());
    if !raised.is_empty() {
        crate::ui_script::safe_world::lend(world, |script| {
            for raise in raised {
                raise.run(script);
            }
        });
    }
    crate::ui_script::live::fire_handler_events(world);
    world.resource_mut::<UiEvents>().put_back(earlier);
    out
}

/// Answer a verb's ask inside the Lua call that made it, over the lent `World`; `None` in a world without the state it reads, where the verb's request waits for its drain. A gate or a filing raises nothing here, and the verb signals what the answer leaves; the bind dialogs' answers run step by step, each raise fired at its own (`0x6e0273`, `0x6e02cd`).
pub(crate) fn answer_in_call(
    lua: &mlua::Lua,
    ask: &benilla_ui::script::HostAsk,
) -> Option<benilla_ui::script::HostAnswer> {
    use benilla_ui::script::{HostAnswer, HostAsk};
    crate::ui_script::safe_world::with_world(|world| {
        match ask.clone() {
        HostAsk::GuildBank(request) => world
            .run_system_cached_with(crate::ui_guild_bank::decide, request)
            .ok()
            .map(HostAnswer::GuildBank),
        HostAsk::BindSwap(mv) => world
            .run_system_cached_with(crate::ui_bind_confirm::file_swap, mv)
            .ok()
            .map(HostAnswer::BindIndex),
        HostAsk::BindAutoEquip { bag, slot } => world
            .run_system_cached_with(crate::ui_bind_confirm::file_auto_equip, (bag, slot))
            .ok()
            .flatten()
            .map(HostAnswer::BindIndex),
        HostAsk::BindAnswers => {
            crate::ui_items::run_bind_answers(world).then_some(HostAnswer::Done)
        }
        // The display's chat arm runs the chat add inside the call (`0x52183c` -> `0x509dd0`);
        // what an enclosing turn queued stays its own.
        HostAsk::ChatLine { chat_type, text } => {
            let earlier = world
                .get_resource_or_init::<crate::ui_script::live::UiEvents>()
                .set_aside();
            crate::ui_chat::add_chat_line_in_call(world, chat_type, text);
            crate::ui_script::live::fire_handler_events(world);
            world
                .resource_mut::<crate::ui_script::live::UiEvents>()
                .put_back(earlier);
            Some(HostAnswer::Done)
        }
        // The active player's object, read at the call (`0x4d3790` -> `0x4d4db0`).
        HostAsk::ActivePlayer => Some(
            if world
                .query_filtered::<(), (
                    With<crate::net::ObjectStore>,
                    With<crate::net::SelfPlayer>,
                )>()
                .iter(world)
                .next()
                .is_some()
            {
                HostAnswer::Done
            } else {
                HostAnswer::Declined
            },
        ),
        // `SendChatMessage` past its argument reads, every line it adds added in the call
        // (`0x50d170`).
        // What an enclosing turn queued stays its own.
        HostAsk::ChatSend(send) => {
            let earlier = world
                .get_resource_or_init::<crate::ui_script::live::UiEvents>()
                .set_aside();
            let refused =
                crate::ui_chat::input::send_chat_message(world, &UiScript::in_call(lua), send);
            crate::ui_script::live::fire_handler_events(world);
            world
                .resource_mut::<crate::ui_script::live::UiEvents>()
                .put_back(earlier);
            Some(refused.map_or(HostAnswer::Done, HostAnswer::LuaError))
        }
        // A channel verb's send inside its call.
        HostAsk::Channel(cmd) => {
            let earlier = world
                .get_resource_or_init::<crate::ui_script::live::UiEvents>()
                .set_aside();
            // The calling VM's handle: the world-entry load runs its addons with the VM out of the
            // `World`, and their joins claim and send as any other's (`0x507c47`, `0x507cc1`).
            let went_on =
                crate::ui_chat::input::run_channel_command(world, UiScript::in_call(lua), cmd);
            crate::ui_script::live::fire_handler_events(world);
            world
                .resource_mut::<crate::ui_script::live::UiEvents>()
                .put_back(earlier);
            Some(if went_on {
                HostAnswer::Done
            } else {
                HostAnswer::Declined
            })
        }
        // The join's profanity test on its typed name, its line shown in the call (`0x50d9c7`,
        // `0x50d9d8`).
        HostAsk::ProfaneChannelName(name) => Some(
            if crate::ui_chat::input::refuse_profane_channel_name(
                world,
                &UiScript::in_call(lua),
                &name,
            ) {
                HostAnswer::Declined
            } else {
                HostAnswer::Done
            },
        ),
        // `guildRecruitmentChannel`'s callback inside its `SetCVar` (`0x51275f` -> `0x508260`).
        HostAsk::GuildRecruitmentMode(mode) => {
            let earlier = world
                .get_resource_or_init::<crate::ui_script::live::UiEvents>()
                .set_aside();
            crate::ui_chat::set_guild_recruitment_mode(world, &mut UiScript::in_call(lua), mode);
            crate::ui_script::live::fire_handler_events(world);
            world
                .resource_mut::<crate::ui_script::live::UiEvents>()
                .put_back(earlier);
            Some(HostAnswer::Done)
        }
        // `SendAddonMessage`'s packet (`0x50071f`).
        HostAsk::AddonSend(send) => {
            crate::ui_chat::input::send_addon_message(world, send);
            Some(HostAnswer::Done)
        }
    }
    })
    .flatten()
}

/// A press that did not run drops its whole press silently: a hard failure in a debug build.
pub(crate) fn step_ran<O, E: std::fmt::Display>(ran: Result<O, E>) -> Option<O> {
    match ran {
        Ok(out) => Some(out),
        Err(e) if cfg!(debug_assertions) => panic!("a press or handler step did not run: {e}"),
        Err(e) => {
            warn!("a press or handler step did not run: {e}");
            None
        }
    }
}

/// A step's raises fire in two lists, the VM's in turn and the handler queue's, so one step never raises on both: a hard failure in a debug build.
pub(crate) fn assert_one_raise_list(world: &World, raised: bool) {
    let queued = world
        .get_resource::<crate::ui_script::live::UiEvents>()
        .is_some_and(|ui| !ui.is_empty());
    debug_assert!(
        !(raised && queued),
        "a step raised both in turn and on the handler queue: their order is lost"
    );
}

/// Apply the calls that waited where no `World` was lent, in call order, in the press pass after
/// the clicks and keys and before the frame's feed, which publishes the cooldowns they arm.
pub(crate) fn apply_script_calls(world: &mut World) {
    crate::ui_script::safe_world::lend(world, |script| {
        benilla_ui::script::apply_queued(script.lua());
    });
}

/// Apply one call in its owner's applier; its edges queue on `ui` and its VM raises queue in turn ([`benilla_ui::script::world_loan::raises_in_turn`]) for the caller to fire once its borrows end.
#[allow(clippy::type_complexity)] // the appliers, one per owner of the state a call touches
fn apply_one(
    In(call): In<ScriptCall>,
    (mut script, merchant, mut ui): (
        NonSendMut<UiScript>,
        Option<Res<crate::ui_merchant::MerchantOpen>>,
        ResMut<crate::ui_script::live::UiEvents>,
    ),
    mut appliers: ParamSet<(
        crate::target::ScriptSelect,
        crate::spell::ScriptCursor,
        crate::spell::SelfCancel,
        crate::spell::ScriptCast,
        crate::ui_action::ActionPress,
        crate::ui_items::ScriptItemUse,
        crate::ui_pet::PetPress,
        crate::ui_action::AttackPress,
    )>,
) -> Applied {
    let script = &mut *script;
    let ui = &mut InTurn(&mut ui);
    let mut applied = Applied::Done;
    {
        match call {
            ScriptCall::Select(request) => appliers.p0().select(request, script),
            ScriptCall::TargetByName { name, exact } => {
                appliers.p0().target_by_name(&name, exact, script);
            }
            ScriptCall::TargetNearest { mode, reverse } => {
                appliers.p0().target_nearest(mode, reverse, script);
            }
            ScriptCall::TargetLastTarget => appliers.p0().target_last_target(script),
            ScriptCall::ClearTarget => appliers.p0().clear_target(),
            ScriptCall::AttackTarget => appliers.p7().attack_target(ui),
            ScriptCall::StartAttack { unit: None, .. } => appliers.p7().start_attack(ui),
            ScriptCall::StartAttack {
                unit: Some(unit),
                exact,
            } => {
                let named = appliers.p0().attack_unit(&unit, exact, script);
                if appliers.p7().may_start(ui) {
                    match named {
                        Some((entity, guid)) => {
                            appliers.p0().select_for_attack(entity, guid, script);
                            appliers.p7().swing_at_selection(ui);
                        }
                        None => crate::ui_action::AttackPress::no_attack_target(ui),
                    }
                }
            }
            ScriptCall::StopAttack => appliers.p7().stop_attack(ui),
            ScriptCall::Dismount => crate::ui_shapeshift::dismount(&mut appliers.p3()),
            ScriptCall::CancelShapeshiftForm => {
                crate::ui_shapeshift::cancel_form(&mut appliers.p3());
            }
            ScriptCall::SpellTargetUnit(token) => {
                appliers.p1().spell_target_unit(&token, script);
            }
            ScriptCall::SpellStopTargeting => appliers.p1().stop_targeting(),
            ScriptCall::PlaceGlyph { socket, verdict } => {
                appliers.p1().place_glyph(socket, verdict);
            }
            ScriptCall::SpellStopCasting => appliers.p2().stop_casting(),
            ScriptCall::CastSpell {
                spell_id,
                at,
                via_book,
            } => {
                crate::ui_spellbook::cast_spell(&mut appliers.p3(), spell_id, at, via_book, script);
            }
            ScriptCall::CastShapeshiftForm(spell_id) => {
                crate::ui_shapeshift::cast_form(&mut appliers.p3(), spell_id);
            }
            ScriptCall::UseAction(press) => {
                match crate::ui_action::use_action(&mut appliers.p4(), script, press) {
                    crate::ui_action::UseOutcome::Done => {}
                    crate::ui_action::UseOutcome::Macro(index) => applied = Applied::Macro(index),
                    crate::ui_action::UseOutcome::Attack => appliers.p7().attack_target(ui),
                    // A pet-action slot sends the pet bar's action, at no unit (`0x5ac000` →
                    // `0x5d4210`).
                    crate::ui_action::UseOutcome::PetAction(index) => {
                        // The pet arm sends a word that holds anything, then runs the bar's
                        // selection signal (`0x5abe32`).
                        let held = appliers
                            .p6()
                            .bar
                            .as_ref()
                            .spells
                            .bar
                            .get(index as usize)
                            .is_some_and(|w| !w.is_empty());
                        let out =
                            pet_press(&mut appliers.p6(), ui, |p, s| p.press_slot(index + 1, s));
                        cast_as_pet(&mut appliers.p3(), out);
                        if held {
                            appliers.p3().ladder.press.defer(ui.0);
                            ui.0.fire("ACTIONBAR_UPDATE_STATE", Vec::new());
                        }
                    }
                }
            }
            // Applied by the caller, which holds no borrow for its view write.
            ScriptCall::DestroyTotem(_) | ScriptCall::Press(_) | ScriptCall::Loot(_) => {}
            ScriptCall::UseItem { bag, slot } => appliers.p5().use_item(script, bag, slot),
            ScriptCall::InteractObject(guid) => appliers.p0().interact_object(guid),
            ScriptCall::UseContainerItem { bag, slot } => {
                // The vendor the bag click sells to (`0xbfa3e8`).
                let vendor = merchant.as_ref().and_then(|m| m.vendor);
                appliers.p5().use_container_item(script, bag, slot, vendor);
            }
            ScriptCall::UseInventoryItem(id) => appliers.p5().use_inventory_item(script, id),
            ScriptCall::UseItemAt { bag, slot, at } => {
                let cast = appliers.p3();
                let aim =
                    cast.targeting
                        .aim(&cast.tokens, &at, crate::ui_unit::VmUnits::of(script));
                appliers.p5().use_item_at(script, bag, slot, aim);
            }
            ScriptCall::EquipItem { bag, slot, dst } => {
                appliers.p5().equip_item(script, bag, slot, dst);
            }
            ScriptCall::CastPetSpell { spell_id, on_self } => {
                let out = pet_press(&mut appliers.p6(), ui, |p, s| {
                    crate::ui_pet_book::cast_pet_spell(p, spell_id, on_self, s)
                });
                cast_as_pet(&mut appliers.p3(), out);
            }
            ScriptCall::PetAction(slot) => {
                let out = pet_press(&mut appliers.p6(), ui, |p, s| p.press_slot(slot, s));
                cast_as_pet(&mut appliers.p3(), out);
            }
            ScriptCall::PetOrder(packed) => {
                let out = pet_press(&mut appliers.p6(), ui, |p, s| p.order(packed, s));
                cast_as_pet(&mut appliers.p3(), out);
            }
        }
        // A harness without the press's step queue holds the attack pick here; with it, the pick and the rungs below it are steps of their own ([`crate::spell::attack_pick_step`]).
        if let Some(held) = appliers.p3().ladder.take_held() {
            if let Some(unit) = appliers.p7().pick_for_cast(held, ui) {
                let mut cast = appliers.p3();
                let ctx = cast
                    .targeting
                    .context_at(crate::spell::cast_target::Aim::Unit(Some(unit)));
                cast.ladder.resume_after_pick(held, &ctx);
            }
        }
        // A press's cast-bar edges, queued in turn for the caller to fire as the call returns
        // (`0x80b556`, `0x806200`); the cancel's share the ladder's queue.
        appliers.p3().ladder.press.defer(ui.0);
    }
    applied
}

/// An applier's sink: its signals and lines queue in turn, a line for the VM's own message display
/// (`0x5216f0`), to fire once the applier's borrows end.
pub(crate) struct InTurn<'a>(pub(crate) &'a mut crate::ui_script::live::UiEvents);

impl crate::ui_action::signals::SignalSink for InTurn<'_> {
    fn signal(&mut self, event: &'static str) {
        self.0.fire(event, Vec::new());
    }

    fn fire(&mut self, event: &'static str, args: Vec<benilla_ui::script::ScriptValue>) {
        self.0.fire(event, args);
    }

    fn unlock_item(&mut self, guid: u64) {
        crate::ui_action::signals::SignalSink::unlock_item(self.0, guid);
    }

    fn line(&mut self, line: crate::ui_action::UiError) {
        self.0
            .write(move |script| crate::ui_action::signals::SignalSink::line(script, line));
    }
}

/// A pet press, then the `PET_BAR_UPDATE`s its latch raised (`0x5d4530`) and what its cooldown start made (`0x805d70`): an on-hold start's usable update (`0x805ed1`), then the pet's cooldown flush (`0x805eda`), in turn on `ui` as the press returns; its `CMSG_PET_ACTION` after their listeners (`0x5d4556`).
pub(crate) fn pet_press(
    press: &mut crate::ui_pet::PetPress,
    ui: &mut InTurn,
    f: impl FnOnce(
        &mut crate::ui_pet::PetPress,
        &mut dyn crate::ui_action::signals::SignalSink,
    ) -> crate::ui_pet::PetCast,
) -> crate::ui_pet::PetCast {
    let at = |press: &crate::ui_pet::PetPress| {
        let bar = press.bar.as_ref();
        (bar.bar_signals, bar.cooldowns.generation)
    };
    let (signals, generation) = at(press);
    let out = f(press, ui);
    let (now_signals, now_generation) = at(press);
    if now_generation != generation {
        press.cooldown_start_in_turn(ui.0);
    }
    crate::ui_pet::bar_signals_in_turn(ui.0, signals, now_signals);
    if let crate::ui_pet::PetCast::Send {
        packed,
        target_guid,
    } = out
    {
        ui.0.world(move |w| {
            if let (Some(bar), Some(c)) = (
                w.get_resource::<crate::ui_pet::PetBar>(),
                w.get_resource::<crate::net::NetCommands>(),
            ) {
                crate::ui_pet::send_pet_action(bar, c, packed, target_guid);
            }
        });
        return crate::ui_pet::PetCast::Done;
    }
    out
}

/// A pet press that forked to the cast ladder (`0x5d445a`): run it with the pet as caster.
fn cast_as_pet(cast: &mut crate::spell::ScriptCast, out: crate::ui_pet::PetCast) {
    if let crate::ui_pet::PetCast::CastAsPet {
        pet_guid,
        spell_id,
        target_guid,
    } = out
    {
        let target = (target_guid != 0).then_some(target_guid);
        let owner_casts = cast
            .ladder
            .spells
            .as_ref()
            .and_then(|s| s.catalog.get(spell_id))
            .is_some_and(|d| d.attributes_ex6 & 0x4_0000 != 0);
        let ctx = cast.targeting.pet_context(pet_guid, target, owner_casts);
        cast.ladder.send_pet(spell_id, &ctx, pet_guid);
    }
}

/// Apply every queued call front to back through `apply`, the calls one queues ahead of the
/// next, as a call applies inside the call that made it.
#[cfg(test)]
pub(crate) fn in_call_order(
    script: &mut UiScript,
    mut apply: impl FnMut(&mut UiScript, ScriptCall),
) {
    let mut queue: std::collections::VecDeque<ScriptCall> = script.take_script_calls().into();
    while let Some(call) = queue.pop_front() {
        apply(script, call);
        for call in script.take_script_calls().into_iter().rev() {
            queue.push_front(call);
        }
    }
}

#[cfg(test)]
mod frame_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::{
        ClientCommand, Guid, GuidIndex, NetCommands, ObjectStore, SelfGuid, SelfPlayer,
    };
    use crate::target::Selection;
    use benilla_protocol::messages::{
        ActionButton, GroupMemberEntry, ACTION_KIND_MACRO, ACTION_KIND_SPELL,
    };
    use benilla_ui::script::{SpellBookState, SpellSlotView, SpellTabView};
    use bevy::ecs::system::RunSystemOnce;
    use crossbeam_channel::Receiver;

    const ME: u64 = 0x10;
    /// The unit selected before the script runs.
    const OLD: u64 = 0x21;
    /// `party1`, the unit the script selects.
    const NEW: u64 = 0x22;
    /// Not in the (absent) spell catalog, so the cast's wire target is the selection as it stands.
    const HEAL: u32 = 2050;
    /// A mob the TAB scan can pick: it alone carries a `NetEntity`.
    const MOB: u64 = 0x31;
    /// `UNIT_FIELD_FACTIONTEMPLATE`, `UNIT_FIELD_HEALTH`, `UNIT_FIELD_FLAGS`.
    const TEMPLATE: u16 = benilla_protocol::field::FIELD_UNIT_FACTIONTEMPLATE;
    const HEALTH: u16 = benilla_protocol::field::FIELD_UNIT_HEALTH;
    const FLAGS: u16 = benilla_protocol::field::FIELD_UNIT_FLAGS;
    /// The fixture's templates: ours (group 1, friendly toward group 2, enemy of group 4), a
    /// friendly NPC's (group 2) and a hostile mob's (group 4).
    const OURS: u32 = 1;
    const FRIEND: u32 = 2;
    const FOE: u32 = 3;

    /// A press step whose system cannot run loses its whole press (its send, its GCD arm): a debug build fails on it.
    #[test]
    #[should_panic(expected = "a press or handler step did not run")]
    fn a_press_step_that_cannot_run_fails_loudly() {
        #[derive(Resource)]
        struct Absent;
        let mut world = World::new();
        press_step(&mut world, |_: In<()>, _: Res<Absent>| {}, ());
    }

    struct Frame {
        app: App,
        rx: Receiver<ClientCommand>,
    }

    /// The whole client, headless, with us, `OLD` (selected when `selected`) and `NEW` as
    /// `party1`, and a VM whose book holds Heal.
    fn frame(selected: bool) -> Frame {
        let mut app = crate::game_plugins::schedule_tests::headless_client();
        let (tx, rx) = crossbeam_channel::unbounded();
        app.insert_resource(NetCommands(tx));
        let world = app.world_mut();
        // Fields 22 health and 28 max health: every unit is alive.
        let unit = |guid: u64, x: f32| {
            (
                Guid(guid),
                Transform::from_xyz(x, 0.0, 0.0),
                GlobalTransform::from_translation(Vec3::new(x, 0.0, 0.0)),
                ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                    (benilla_protocol::field::FIELD_UNIT_HEALTH, 100),
                    (benilla_protocol::field::FIELD_UNIT_MAXHEALTH, 100),
                ])),
            )
        };
        // We are a player (`OBJECT_FIELD_TYPE` 0x19, `UNIT_FLAG_PVP_ATTACKABLE`), which selects The
        // attack check's player arm: with no `FactionTemplate.dbc` a plain unit is attackable.
        let me = world.spawn((SelfPlayer, unit(ME, 0.0))).id();
        world
            .entity_mut(me)
            .insert(ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                (benilla_protocol::field::FIELD_OBJECT_TYPE, 0x19),
                (benilla_protocol::field::FIELD_UNIT_HEALTH, 100),
                (benilla_protocol::field::FIELD_UNIT_MAXHEALTH, 100),
                (benilla_protocol::field::FIELD_UNIT_FLAGS, 0x8),
            ])));
        let old = world.spawn(unit(OLD, 5.0)).id();
        let new = world.spawn(unit(NEW, 6.0)).id();
        let index = &mut world.resource_mut::<GuidIndex>().0;
        index.insert(ME, me);
        index.insert(OLD, old);
        index.insert(NEW, new);
        world.resource_mut::<SelfGuid>().0 = Some(ME);
        world
            .resource_mut::<crate::ui_party::GroupState>()
            .members
            .push(GroupMemberEntry {
                name: "Bob".into(),
                guid: NEW,
                status: 1,
                flags: 0,
                roles: 0,
            });
        if selected {
            *world.resource_mut::<Selection>() = Selection {
                target: Some(old),
                guid: Some(OLD),
                ..Default::default()
            };
        }
        let mut script = UiScript::new().expect("a VM");
        world.insert_resource(crate::ui_spellbook::SpellBook::of(SpellBookState {
            tabs: vec![SpellTabView {
                name: "Holy".into(),
                texture: None,
                offset: 0,
                num_spells: 1,
            }],
            slots: vec![SpellSlotView {
                spell_id: HEAL,
                name: "Heal".into(),
                ..Default::default()
            }],
            ..Default::default()
        }));
        if selected {
            // The frame's unit snapshot, as the feed pushed it before the input pass.
            script.set_unit(
                "target",
                Some(benilla_ui::script::UnitState {
                    exists: true,
                    ..Default::default()
                }),
            );
        }
        world.insert_non_send_resource(script);
        Frame { app, rx }
    }

    /// Run `lua` in the VM, then this frame's one application of what it queued.
    fn run(f: &mut Frame, lua: &str) {
        crate::ui_script::live::with_live_world(f.app.world_mut(), |s| {
            s.run(lua).expect("the script runs")
        })
        .expect("the frame's VM");
        f.app
            .world_mut()
            .run_system_once(apply_script_calls)
            .expect("the calls apply as a one-shot system");
    }

    /// The `CMSG_CAST_SPELL`s sent, as `(spell, target)`.
    fn casts(f: &Frame) -> Vec<(u32, Option<u64>)> {
        f.rx.try_iter()
            .filter_map(|c| match c {
                ClientCommand::CastSpell {
                    spell_id, target, ..
                } => Some((spell_id, target)),
                _ => None,
            })
            .collect()
    }

    fn selected(f: &Frame) -> Option<u64> {
        f.app.world().resource::<Selection>().guid
    }

    /// `MOB` at 3 yd, alive, with `fields` over its health.
    fn spawn_mob(f: &mut Frame, fields: &[(u16, u32)]) {
        let world = f.app.world_mut();
        let mut pairs = vec![
            (HEALTH, 100),
            (benilla_protocol::field::FIELD_UNIT_MAXHEALTH, 100),
        ];
        pairs.extend_from_slice(fields);
        let mob = world
            .spawn((
                Guid(MOB),
                crate::net::NetEntity {
                    kind: benilla_protocol::EntityKind::Unit,
                    display_id: None,
                    scale: 1.0,
                },
                Transform::from_xyz(3.0, 0.0, 0.0),
                GlobalTransform::from_translation(Vec3::new(3.0, 0.0, 0.0)),
                ObjectStore(benilla_protocol::ObjectFields::from_pairs(&pairs)),
            ))
            .id();
        world.resource_mut::<GuidIndex>().0.insert(MOB, mob);
    }

    /// Give the world a faction table, us [`OURS`] and `OLD` the `old` template with `fields`
    /// over it.
    fn with_factions(f: &mut Frame, old: u32, fields: &[(u16, u32)]) {
        use benilla_formats::{FactionCatalog, FactionTemplate};
        use std::collections::HashMap;
        let tpl = |faction, group_mask, friend_group_mask, enemy_group_mask| FactionTemplate {
            faction,
            group_mask,
            friend_group_mask,
            enemy_group_mask,
            enemies: [0; 4],
            friends: [0; 4],
        };
        let world = f.app.world_mut();
        world.insert_resource(crate::target::Factions::from_catalog(
            FactionCatalog::from_rows(
                HashMap::from([
                    (OURS, tpl(OURS, 1, 2, 4)),
                    (FRIEND, tpl(FRIEND, 2, 0, 0)),
                    (FOE, tpl(FOE, 4, 0, 0)),
                ]),
                HashMap::new(),
            ),
        ));
        let index = &world.resource::<GuidIndex>().0;
        let (me, old_unit) = (index[&ME], index[&OLD]);
        world
            .entity_mut(me)
            .insert(ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                (benilla_protocol::field::FIELD_OBJECT_TYPE, 0x19),
                (HEALTH, 100),
                (benilla_protocol::field::FIELD_UNIT_MAXHEALTH, 100),
                (FLAGS, 0x8),
                (TEMPLATE, OURS),
            ])));
        let mut pairs = vec![
            (HEALTH, 100),
            (benilla_protocol::field::FIELD_UNIT_MAXHEALTH, 100),
            (TEMPLATE, old),
        ];
        pairs.extend_from_slice(fields);
        world
            .entity_mut(old_unit)
            .insert(ObjectStore(benilla_protocol::ObjectFields::from_pairs(
                &pairs,
            )));
    }

    /// The `CMSG_ATTACKSWING`s sent, and whether a `CMSG_ATTACKSTOP` went out.
    fn swings(f: &Frame) -> (Vec<u64>, bool) {
        let mut stopped = false;
        let swings =
            f.rx.try_iter()
                .filter_map(|c| match c {
                    ClientCommand::AttackSwing { guid } => Some(guid),
                    ClientCommand::AttackStop => {
                        stopped = true;
                        None
                    }
                    _ => None,
                })
                .collect();
        (swings, stopped)
    }

    /// The keys of the error lines raised.
    /// The keys of the lines shown, taken.
    fn errors(f: &mut Frame) -> Vec<&'static str> {
        f.app
            .world_mut()
            .non_send_resource_mut::<benilla_ui::script::UiScript>()
            .take_shown_keys()
    }

    /// `CastSpellByName(name, unit)` (`0x540310`) casts at the unit its token names and
    /// `CastSpell(slot, book, onSelf)` (`0x541250`) at the active player under the flag, whoever is
    /// selected, and the selection stays where it was. Without either it goes to the selection; a
    /// token the resolver does not know casts nothing.
    #[test]
    fn a_unit_argument_aims_the_cast_and_leaves_the_selection() {
        for (lua, target, selection) in [
            (r#"CastSpellByName("Heal", "player")"#, Some(ME), OLD),
            (r#"CastSpell(1, "spell", 1)"#, Some(ME), OLD),
            (r#"CastSpellByName("Heal", "party1")"#, Some(NEW), OLD),
            (
                r#"TargetUnit("party1") CastSpellByName("Heal", "player")"#,
                Some(ME),
                NEW,
            ),
            (r#"CastSpellByName("Heal")"#, Some(OLD), OLD),
            (r#"CastSpellByName("Heal", "")"#, Some(OLD), OLD),
            (r#"CastSpellByName("Heal", 1)"#, None, OLD),
            (r#"CastSpell(1, "spell")"#, Some(OLD), OLD),
            (r#"CastSpell(1, "spell", "0")"#, Some(OLD), OLD),
        ] {
            let mut f = frame(true);
            run(&mut f, lua);
            let sent: Vec<_> = target.into_iter().map(|t| (HEAL, Some(t))).collect();
            assert_eq!(casts(&f), sent, "{lua}");
            assert_eq!(selected(&f), Some(selection), "{lua}");
        }
    }

    /// Power Word: Fortitude's word, an assist unit (implicit target 21), in the catalog and
    /// the book.
    const FORTITUDE: u32 = 1243;

    fn with_fortitude(f: &mut Frame) {
        let world = f.app.world_mut();
        let display = benilla_formats::SpellDisplay {
            implicit_target_a1: 21,
            ..Default::default()
        };
        world.insert_resource(crate::ui_action::Spells {
            catalog: benilla_formats::SpellCatalog::from_displays([(FORTITUDE, display)].into()),
            ..crate::ui_action::Spells::empty_for_tests()
        });
        world.insert_resource(crate::ui_spellbook::SpellBook::of(SpellBookState {
            tabs: vec![SpellTabView {
                name: "Priest".into(),
                texture: None,
                offset: 0,
                num_spells: 1,
            }],
            slots: vec![SpellSlotView {
                spell_id: FORTITUDE,
                name: "Fortitude".into(),
                ..Default::default()
            }],
            ..Default::default()
        }));
    }

    /// The bind runs the ordinary relation chain over the player: with a unit we cannot assist
    /// selected, the plain call binds us behind `autoSelfCast` (12340's default on) and nothing
    /// with it off (the cursor comes up), and `"player"` binds us either way; with a friendly
    /// player selected the plain call binds that player.
    #[test]
    fn the_player_binds_where_the_selection_is_not_assistable() {
        for (selected_template, auto, lua, target) in [
            (FOE, true, r#"CastSpellByName("Fortitude")"#, Some(ME)),
            (FOE, false, r#"CastSpellByName("Fortitude")"#, None),
            (
                FOE,
                false,
                r#"CastSpellByName("Fortitude", "player")"#,
                Some(ME),
            ),
            (FOE, false, r#"CastSpell(1, "spell", 1)"#, Some(ME)),
            (FRIEND, true, r#"CastSpellByName("Fortitude")"#, Some(OLD)),
            (
                FRIEND,
                true,
                r#"CastSpellByName("Fortitude", "player")"#,
                Some(ME),
            ),
        ] {
            let mut f = frame(true);
            f.app
                .world_mut()
                .insert_resource(crate::spell::AutoSelfCast(auto));
            // The assist check passes a friendly player-controlled unit
            // (`UNIT_FLAG_PLAYER_CONTROLLED`).
            with_factions(&mut f, selected_template, &[(FLAGS, 0x8)]);
            with_fortitude(&mut f);
            run(&mut f, lua);
            let sent: Vec<_> = target.into_iter().map(|t| (FORTITUDE, Some(t))).collect();
            assert_eq!(
                casts(&f),
                sent,
                "{lua} at template {selected_template}, autoSelfCast {auto}"
            );
            assert_eq!(selected(&f), Some(OLD), "{lua}");
        }
    }

    /// The pet's guid, and the pet book's Growl (`CMSG_PET_ACTION`'s type-1 word).
    const PET: u64 = 0x50;
    const GROWL: u32 = 2649;

    /// The `CMSG_PET_ACTION`s sent, as `(pet, word, target)`.
    fn pet_actions(f: &Frame) -> Vec<(u64, u32, u64)> {
        f.rx.try_iter()
            .filter_map(|c| match c {
                ClientCommand::PetAction {
                    pet_guid,
                    packed,
                    target_guid,
                } => Some((pet_guid, packed, target_guid)),
                _ => None,
            })
            .collect()
    }

    /// The book's pet cast (`0x53bc40`) hands the pet-action send the guid it was given, so
    /// `CastSpell(slot, "pet", 1)` aims the pet's spell at the player, not the selection.
    #[test]
    fn a_pet_book_cast_on_self_aims_at_the_player() {
        for (lua, target) in [
            (r#"CastSpell(1, "pet", 1)"#, ME),
            (r#"CastSpell(1, "pet", "on")"#, ME),
            (r#"CastSpell(1, "pet")"#, OLD),
            (r#"CastSpell(1, "pet", "0")"#, OLD),
        ] {
            let mut f = frame(true);
            {
                let world = f.app.world_mut();
                let mut bar = world.resource_mut::<crate::ui_pet::PetBar>();
                bar.spells.pet_guid = PET;
                // The book's one word, the array the verbs read at the call.
                bar.spells.spells = vec![benilla_protocol::messages::PetActionEntry::from(
                    GROWL | (u32::from(benilla_protocol::messages::PET_ACT_ENABLED) << 24),
                )];
            }
            // Our own pet, `SUMMONEDBY` us, so the usability gate passes (`0x5d4285`).
            let pet = f
                .app
                .world_mut()
                .spawn((
                    Guid(PET),
                    ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                        (benilla_protocol::field::FIELD_UNIT_SUMMONEDBY, ME as u32),
                        (benilla_protocol::field::FIELD_UNIT_SUMMONEDBY + 1, 0),
                    ])),
                ))
                .id();
            f.app
                .world_mut()
                .resource_mut::<GuidIndex>()
                .0
                .insert(PET, pet);
            // The spell arm leaves without a `Spell.dbc` record (`0x5d42b4`).
            f.app.world_mut().insert_resource(crate::ui_action::Spells {
                catalog: benilla_formats::SpellCatalog::from_displays(
                    [(GROWL, benilla_formats::SpellDisplay::default())].into(),
                ),
                ..crate::ui_action::Spells::empty_for_tests()
            });
            run(&mut f, lua);
            assert_eq!(
                pet_actions(&f),
                vec![(PET, 0x0100_0000 | GROWL, target)],
                "{lua}"
            );
            assert_eq!(selected(&f), Some(OLD), "{lua}");
        }
    }

    /// `/target` then `/cast`: `TargetUnit` commits through the selection setter before it returns,
    /// and `CastSpellByName` casts at the selection as it stands then.
    #[test]
    fn a_cast_after_a_target_goes_to_the_new_target() {
        let mut f = frame(true);
        run(&mut f, r#"TargetUnit("party1") CastSpellByName("Heal")"#);
        assert_eq!(casts(&f), vec![(HEAL, Some(NEW))]);
        assert_eq!(selected(&f), Some(NEW));
    }

    /// `/cast` then `/target`: the cast is done before the selection moves.
    #[test]
    fn a_cast_before_a_target_goes_to_the_old_target() {
        let mut f = frame(true);
        run(&mut f, r#"CastSpellByName("Heal") TargetUnit("party1")"#);
        assert_eq!(casts(&f), vec![(HEAL, Some(OLD))]);
        assert_eq!(selected(&f), Some(NEW));
    }

    /// `ClearTarget` reads the selection at call time and deselects: after `TargetUnit("player")`
    /// there is one to clear, with or without a target before the script ran.
    #[test]
    fn a_clear_after_a_target_ends_with_nothing_selected() {
        for selected_before in [true, false] {
            let mut f = frame(selected_before);
            run(&mut f, r#"TargetUnit("player") ClearTarget()"#);
            assert_eq!(selected(&f), None, "selected before: {selected_before}");
            let world = f.app.world();
            assert_eq!(world.resource::<Selection>().target, None);
            let sends: Vec<u64> =
                f.rx.try_iter()
                    .filter_map(|c| match c {
                        ClientCommand::SetSelection { guid } => Some(guid),
                        _ => None,
                    })
                    .collect();
            assert_eq!(sends, vec![ME, 0], "selected before: {selected_before}");
        }
    }

    /// `TargetNearestEnemy` commits through the selection setter before it returns, so the cast
    /// after it in the same script goes to the unit the TAB cycle picked.
    #[test]
    fn a_cast_after_target_nearest_enemy_goes_to_the_picked_unit() {
        let mut f = frame(true);
        // OLD and NEW carry no `NetEntity`, so the scan's one candidate is the mob.
        spawn_mob(&mut f, &[]);
        run(&mut f, r#"TargetNearestEnemy() CastSpellByName("Heal")"#);
        assert_eq!(casts(&f), vec![(HEAL, Some(MOB))]);
        assert_eq!(selected(&f), Some(MOB));
    }

    /// `TargetLastTarget` reads the pair the selection setter stamps with the outgoing selection: a
    /// held pair re-selects, which swaps the two; an empty one deselects, which stamps what it
    /// drops.
    #[test]
    fn target_last_target_swaps_and_deselects_on_an_empty_pair() {
        let mut f = frame(false);
        // Selecting from nothing stamps nothing: the pair is empty, so the call deselects.
        run(&mut f, r#"TargetUnit("party1") TargetLastTarget()"#);
        assert_eq!(selected(&f), None);
        // The deselect stamped NEW, so the next call brings it back.
        run(&mut f, "TargetLastTarget()");
        assert_eq!(selected(&f), Some(NEW));
        // A switch stamps the outgoing unit; the call swaps back, and the cast after it in the
        // same script goes to the unit it re-selected.
        run(
            &mut f,
            r#"TargetUnit("player") TargetLastTarget() CastSpellByName("Heal")"#,
        );
        assert_eq!(selected(&f), Some(NEW));
        assert_eq!(casts(&f), vec![(HEAL, Some(NEW))]);
        run(&mut f, "TargetLastTarget()");
        assert_eq!(selected(&f), Some(ME), "and back again");
    }

    /// `AttackTarget` swings at the selection as the calls before it left it.
    #[test]
    fn attack_target_swings_at_the_selection_in_call_order() {
        let mut f = frame(true);
        run(&mut f, r#"AttackTarget() TargetUnit("party1")"#);
        assert_eq!(swings(&f), (vec![OLD], false));
        let mut f = frame(true);
        run(&mut f, r#"TargetUnit("party1") AttackTarget()"#);
        assert_eq!(swings(&f), (vec![NEW], false));
    }

    /// A mounted Attack refuses past melee reach ahead of the final gate (`0x72bdb0`), and inside
    /// it the dismount leads the swing (`0x6e4950`).
    #[test]
    fn a_mounted_attack_is_refused_past_melee_and_dismounts_inside_it() {
        use benilla_protocol::field::FIELD_UNIT_MOUNTDISPLAYID;
        for x in [20.0, 4.0] {
            let mut f = frame(true);
            with_factions(&mut f, FOE, &[]);
            let world = f.app.world_mut();
            let index = &world.resource::<GuidIndex>().0;
            let (me, old) = (index[&ME], index[&OLD]);
            world.entity_mut(old).insert((
                Transform::from_xyz(x, 0.0, 0.0),
                crate::net::NetEntity {
                    kind: benilla_protocol::EntityKind::Unit,
                    display_id: None,
                    scale: 1.0,
                },
            ));
            world
                .entity_mut(me)
                .insert(ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                    (benilla_protocol::field::FIELD_OBJECT_TYPE, 0x19),
                    (HEALTH, 100),
                    (benilla_protocol::field::FIELD_UNIT_MAXHEALTH, 100),
                    (FLAGS, 0x8),
                    (TEMPLATE, OURS),
                    (FIELD_UNIT_MOUNTDISPLAYID, 1147),
                ])));
            run(&mut f, "AttackTarget()");
            let sent: Vec<ClientCommand> =
                f.rx.try_iter()
                    .filter(|c| {
                        matches!(
                            c,
                            ClientCommand::CancelMountAura | ClientCommand::AttackSwing { .. }
                        )
                    })
                    .collect();
            if x > 5.0 {
                assert_eq!(errors(&mut f), vec!["ERR_OUT_OF_RANGE"]);
                assert!(sent.is_empty());
            } else {
                assert!(errors(&mut f).is_empty());
                assert!(matches!(
                    sent.as_slice(),
                    [
                        ClientCommand::CancelMountAura,
                        ClientCommand::AttackSwing { guid: OLD }
                    ]
                ));
            }
        }
    }

    /// A unit we are friendly toward is dropped and `TargetNearestEnemy` runs, moving the
    /// selection; the swing goes at what it picked. The Attack button reaches the same through the
    /// cast attempt.
    #[test]
    fn attack_with_a_friend_selected_swings_at_the_nearest_enemy_instead() {
        for lua in ["AttackTarget()", "UseAction(1)"] {
            let mut f = frame(true);
            with_factions(&mut f, FRIEND, &[]);
            spawn_mob(&mut f, &[(TEMPLATE, FOE)]);
            f.app
                .world_mut()
                .resource_mut::<crate::ui_action::ActionSlots>()
                .buttons
                .insert(
                    0,
                    ActionButton {
                        slot: 0,
                        action: crate::ui_action::SPELL_ATTACK,
                        kind: benilla_protocol::messages::ACTION_KIND_SPELL,
                    },
                );
            run(&mut f, lua);
            assert_eq!(selected(&f), Some(MOB), "{lua}");
            assert_eq!(swings(&f), (vec![MOB], false), "{lua}");
            assert_eq!(errors(&mut f), Vec::<&str>::new(), "{lua}");
        }
    }

    /// The retarget commits before `AttackTarget` returns, so a cast after it in the same script
    /// goes to the enemy it picked, from a friend or from nothing selected.
    #[test]
    fn a_cast_after_attack_target_goes_to_the_enemy_it_picked() {
        for selected_before in [true, false] {
            let mut f = frame(selected_before);
            with_factions(&mut f, FRIEND, &[]);
            spawn_mob(&mut f, &[(TEMPLATE, FOE)]);
            run(&mut f, r#"AttackTarget() CastSpellByName("Heal")"#);
            assert_eq!(
                casts(&f),
                vec![(HEAL, Some(MOB))],
                "selected before: {selected_before}"
            );
        }
    }

    /// With no enemy in reach the scan moves nothing, so the selection re-read is the friend, which
    /// the final gate's attack check refuses; only an empty selection is "There is nothing to
    /// attack.". Neither swings.
    #[test]
    fn attack_with_no_enemy_near_refuses_by_what_is_selected() {
        let mut f = frame(true);
        with_factions(&mut f, FRIEND, &[]);
        run(&mut f, "AttackTarget()");
        assert_eq!(selected(&f), Some(OLD));
        assert_eq!(errors(&mut f), vec!["ERR_INVALID_ATTACK_TARGET"]);
        assert_eq!(swings(&f), (vec![], false));

        let mut f = frame(false);
        with_factions(&mut f, FRIEND, &[]);
        run(&mut f, "AttackTarget()");
        assert_eq!(selected(&f), None);
        assert_eq!(errors(&mut f), vec!["ERR_NO_ATTACK_TARGET"]);
        assert_eq!(swings(&f), (vec![], false));
    }

    /// A hostile corpse is kept, so no scan runs past it to the live mob, and the final gate
    /// refuses a unit neither alive nor feigning: "You cannot attack that target.", and no swing,
    /// which vmangos would stop (`CombatHandler.cpp:53-58`).
    #[test]
    fn attack_at_a_corpse_cannot_attack_it() {
        let mut f = frame(true);
        with_factions(&mut f, FOE, &[(HEALTH, 0)]);
        spawn_mob(&mut f, &[(TEMPLATE, FOE)]);
        run(&mut f, "AttackTarget()");
        assert_eq!(selected(&f), Some(OLD));
        assert_eq!(errors(&mut f), vec!["ERR_INVALID_ATTACK_TARGET"]);
        assert_eq!(swings(&f), (vec![], false));
    }

    /// The reference's Attack toggle acts only past the validator: attacking a live foe stops, and
    /// a press the validator refuses neither stops nor swings.
    #[test]
    fn attack_toggles_off_only_past_the_validator() {
        for (health, stopped, refused) in [
            (100, true, None),
            (0, false, Some("ERR_INVALID_ATTACK_TARGET")),
        ] {
            let mut f = frame(true);
            with_factions(&mut f, FOE, &[(HEALTH, health)]);
            let world = f.app.world_mut();
            let me = world.resource::<GuidIndex>().0[&ME];
            world
                .entity_mut(me)
                .insert(crate::creature_anim::Engaged(OLD));
            run(&mut f, "AttackTarget()");
            assert_eq!(swings(&f), (vec![], stopped), "health {health}");
            assert_eq!(
                errors(&mut f),
                refused.into_iter().collect::<Vec<_>>(),
                "health {health}"
            );
        }
    }

    /// A macro's lines run inside `UseAction`, so what they call lands before the calls the
    /// pressing script makes after it: the macro's cast goes to the old target, then the script's
    /// `TargetUnit` moves the selection.
    #[test]
    fn a_macros_calls_land_inside_the_use_action_that_ran_it() {
        let mut f = frame(true);
        {
            let world = f.app.world_mut();
            let script = world.non_send_resource::<UiScript>();
            // `ChatFrame1`'s `EXECUTE_CHAT_LINE` handler (`ChatFrame.lua:1343`) sends the line
            // through the edit box; this one runs it as Lua, which is all the order needs.
            script
                .run(
                    r#"local f = CreateFrame("Frame")
                    f:RegisterEvent("EXECUTE_CHAT_LINE")
                    f:SetScript("OnEvent", function(self, event, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9) RunScript(arg1) end)
                    MACRO_INDEX = CreateMacro("heal", 1, 'CastSpellByName("Heal")')"#,
                )
                .expect("the macro is made");
            let index: u32 = script.eval("return MACRO_INDEX").expect("an index");
            world
                .resource_mut::<crate::ui_action::ActionSlots>()
                .buttons
                .insert(
                    0,
                    ActionButton {
                        slot: 0,
                        action: index,
                        kind: ACTION_KIND_MACRO,
                    },
                );
        }
        run(&mut f, r#"UseAction(1) TargetUnit("party1")"#);
        assert_eq!(casts(&f), vec![(HEAL, Some(OLD))]);
        assert_eq!(selected(&f), Some(NEW));
    }

    /// Sinister Strike: `AttributesEx & 0x200`, one of the bits a reference cast predicate tests,
    /// and the enemy word (implicit target 6).
    const SINISTER_STRIKE: u32 = 1752;

    /// Sinister Strike in the catalog and on action slot 1.
    fn with_strike(f: &mut Frame) {
        let world = f.app.world_mut();
        let strike = benilla_formats::SpellDisplay {
            attributes_ex: 0x200,
            implicit_target_a1: 6,
            ..Default::default()
        };
        world.insert_resource(crate::ui_action::Spells {
            catalog: benilla_formats::SpellCatalog::from_displays(
                [(SINISTER_STRIKE, strike)].into(),
            ),
            ..crate::ui_action::Spells::empty_for_tests()
        });
        world
            .resource_mut::<crate::ui_action::ActionSlots>()
            .buttons
            .insert(
                0,
                ActionButton {
                    slot: 0,
                    action: SINISTER_STRIKE,
                    kind: ACTION_KIND_SPELL,
                },
            );
    }

    /// The selections, strikes and swings sent, in wire order.
    fn strike_wire(f: &Frame) -> Vec<(&'static str, u64)> {
        f.rx.try_iter()
            .filter_map(|c| match c {
                ClientCommand::SetSelection { guid } => Some(("select", guid)),
                ClientCommand::CastSpell {
                    spell_id: SINISTER_STRIKE,
                    target: Some(guid),
                    ..
                } => Some(("cast", guid)),
                ClientCommand::AttackSwing { guid } => Some(("swing", guid)),
                _ => None,
            })
            .collect()
    }

    /// The cast attempt's attack pick: a strike pressed with no target selects the nearest enemy
    /// before its bind, so the selection goes out ahead of the cast and its swing, and the call
    /// after it reads the selection only once the cast is done.
    #[test]
    fn a_strike_with_no_target_selects_the_nearest_enemy_and_casts_at_it() {
        let mut f = frame(false);
        spawn_mob(&mut f, &[]);
        with_strike(&mut f);
        run(&mut f, r#"UseAction(1) TargetUnit("party1")"#);
        assert_eq!(
            strike_wire(&f),
            vec![
                ("select", MOB),
                ("cast", MOB),
                ("swing", MOB),
                ("select", NEW)
            ]
        );
    }

    /// A typed `/script` line runs with the `World` lent, so its `TargetUnit` is applied inside
    /// the line and the line's next statement reads the new target (`0x524bf0`).
    #[test]
    fn a_script_lines_target_applies_inside_the_line() {
        let mut f = frame(false);
        // The stock `/script` row (`ChatFrame.lua`), which runs the rest of the line.
        f.app
            .world()
            .non_send_resource::<UiScript>()
            .run(
                r#"SlashCmdList = { SCRIPT = function(msg) loadstring(msg)() end }
                   SLASH_SCRIPT1 = "/script""#,
            )
            .expect("the row is declared");
        f.app
            .world_mut()
            .insert_resource(crate::ui_chat::commands::SlashCommands::default());
        f.app
            .world_mut()
            .non_send_resource_mut::<UiScript>()
            .push_chat_input(
                r#"/script TargetUnit("party1") Seen = UnitExists("target") and 1 or 0"#.into(),
            );
        f.app
            .world_mut()
            .run_system_once(crate::ui_chat::drain_chat_input)
            .expect("the drain runs as a one-shot system");
        let seen = f
            .app
            .world()
            .non_send_resource::<UiScript>()
            .eval::<i64>("return Seen")
            .unwrap();
        assert_eq!((seen, selected(&f)), (1, Some(NEW)));
    }

    /// The pick's selection is told before the cast it makes: `PLAYER_TARGET_CHANGED` fires inside
    /// the setter (`0x524f54`) the pick calls (`0x80d661`), ahead of the send's `UNIT_SPELLCAST_SENT`
    /// (`0x80b556`), and the send's listener reads the new target.
    #[test]
    fn a_strikes_pick_tells_the_new_target_before_the_send() {
        let mut f = frame(false);
        spawn_mob(&mut f, &[]);
        with_strike(&mut f);
        run(
            &mut f,
            r#"Seen = {}
               local l = CreateFrame("Frame")
               l:RegisterEvent("PLAYER_TARGET_CHANGED")
               l:RegisterEvent("UNIT_SPELLCAST_SENT")
               l:SetScript("OnEvent", function(_, e)
                   table.insert(Seen, e .. ":" .. tostring(UnitExists("target") and 1 or 0))
               end)
               UseAction(1)"#,
        );
        let seen = f
            .app
            .world()
            .non_send_resource::<UiScript>()
            .eval::<String>("return table.concat(Seen, ',')")
            .unwrap();
        assert_eq!(seen, "PLAYER_TARGET_CHANGED:1,UNIT_SPELLCAST_SENT:1");
    }

    /// A self-cast press passes the caster to the pick, and the reaction to oneself is 4, so the
    /// pick Tabs to the enemy. With no faction table the caster would read neutral and be kept.
    #[test]
    fn a_self_cast_strike_selects_the_nearest_enemy() {
        let mut f = frame(false);
        spawn_mob(&mut f, &[]);
        with_strike(&mut f);
        run(&mut f, r#"UseAction(1, "player")"#);
        assert_eq!(
            strike_wire(&f),
            vec![("select", MOB), ("cast", MOB), ("swing", MOB)]
        );
    }

    /// The pick's keep test drops a friend, so a strike at one retargets the nearest enemy, where
    /// the bind alone would refuse it as an invalid target.
    #[test]
    fn a_strike_at_a_friend_retargets_the_nearest_enemy() {
        let mut f = frame(true);
        with_factions(&mut f, FRIEND, &[]);
        spawn_mob(&mut f, &[(TEMPLATE, FOE)]);
        with_strike(&mut f);
        run(&mut f, "UseAction(1)");
        assert_eq!(
            strike_wire(&f),
            vec![("select", MOB), ("cast", MOB), ("swing", MOB)]
        );
    }

    /// With no enemy to acquire, the strike stops at the pick with "There is nothing to attack.",
    /// not the bind's "You have no target.".
    #[test]
    fn a_strike_with_nothing_to_attack_says_so_and_sends_nothing() {
        let mut f = frame(false);
        with_strike(&mut f);
        run(&mut f, "UseAction(1)");
        assert_eq!(casts(&f), vec![]);
        assert_eq!(selected(&f), None);
        assert_eq!(errors(&mut f), vec!["ERR_NO_ATTACK_TARGET"]);
        assert!(crate::ui_action::take_refusals(f.app.world_mut()).is_empty());
    }

    /// The pick's target setter fires inside the attempt before the rungs below it (`0x80d661` -> `0x72bdb0` -> `0x524bf0`, then `0x80d6ad`): a `PLAYER_TARGET_CHANGED` listener reads the strike not current and its select goes out first.
    #[test]
    fn a_strikes_pick_listener_runs_before_the_rest_of_the_attempt() {
        let mut f = frame(false);
        spawn_mob(&mut f, &[]);
        with_strike(&mut f);
        run(
            &mut f,
            r#"local l = CreateFrame("Frame")
               l:RegisterEvent("PLAYER_TARGET_CHANGED")
               l:SetScript("OnEvent", function()
                   if AT_PICK then return end
                   AT_PICK = IsCurrentAction(1) and 1 or 0
                   TargetUnit("party1")
               end)
               UseAction(1)"#,
        );
        let at_pick = f
            .app
            .world()
            .non_send_resource::<UiScript>()
            .eval::<i64>("return AT_PICK")
            .unwrap();
        assert_eq!(at_pick, 0, "the strike is not current at its pick's signal");
        // The attempt's guid argument stays the unit the pick bound (`0x80d65e`).
        assert_eq!(
            strike_wire(&f),
            vec![
                ("select", MOB),
                ("select", NEW),
                ("cast", MOB),
                ("swing", MOB)
            ]
        );
    }

    /// A strike held at its pick takes one `castCount`, at its attempt's top (`0x80cda8` -> `0x8050dc`); its rest below the pick keeps it.
    #[test]
    fn a_picked_strike_takes_one_count() {
        let mut f = frame(false);
        spawn_mob(&mut f, &[]);
        with_strike(&mut f);
        let next = f
            .app
            .world()
            .resource::<crate::spell::PendingCast>()
            .next_count();
        run(&mut f, "UseAction(1)");
        let counts: Vec<u8> =
            f.rx.try_iter()
                .filter_map(|c| match c {
                    ClientCommand::CastSpell {
                        spell_id: SINISTER_STRIKE,
                        count,
                        ..
                    } => Some(count),
                    _ => None,
                })
                .collect();
        assert_eq!(counts, [next]);
        assert_eq!(
            f.app
                .world()
                .resource::<crate::spell::PendingCast>()
                .next_count(),
            next.wrapping_add(1).max(1),
            "one count taken"
        );
    }
}
