//! The pet's packet handlers: `SMSG_PET_SPELLS` and `SMSG_PET_MODE` write [`PetBar`]; the refusals
//! show in the handler's turn, the tame failure and the pet's sounds go to their queues. The pet's
//! refusals share the player's red line but not its message tables: the reference maps them in
//! their own handlers to say "your pet" where the player's say "you".

use std::time::Instant;

use bevy::prelude::*;

use benilla_protocol::messages::{PetMode, PetSpells};
use benilla_protocol::{SessionEvent, SessionEventKind};
use benilla_ui::names::NameError;

use benilla_assets::coords::wow_to_bevy;

use super::PetBar;
use crate::net::{GuidIndex, NetHandlerApp, PetDismissSoundMessage, PetTalkMessage};
use crate::ui_action::{CastErrors, PetTameFailures, Spells, UiError};

pub(super) fn register(app: &mut App) {
    use SessionEventKind as K;
    app.net_handler(K::PetSpells, on_spells)
        .net_handler(K::PetMode, on_mode)
        .net_handler(K::PetActionFeedback, on_action_feedback)
        .net_handler(K::PetCastFailed, on_cast_failed)
        .net_handler(K::PetTameFailure, on_tame_failure)
        .net_handler(K::PetNameInvalid, on_refusal_line)
        .net_handler(K::PetBroken, on_refusal_line)
        .net_handler(K::PetActionSound, on_action_sound)
        .net_handler(K::PetDismissSound, on_dismiss_sound)
        .net_handler(K::PetLearnedSpell, on_spell_line)
        .net_handler(K::PetUnlearnedSpell, on_spell_line)
        .net_handler(K::PetRenameable, on_renameable)
        .net_handler(K::PetGuids, on_guids)
        .net_handler(K::Disconnected, on_session_end)
        .net_handler(K::LoggedOut, on_left_world);
}

/// Leaving the world drops the possess spell as the world leave's pet part does (`0x5d3360`), and a world entry's pet step not yet run.
fn on_left_world(
    In(ev): In<SessionEvent>,
    mut possess: ResMut<super::possess::PossessSpell>,
    owed: Option<ResMut<super::possess::WorldEntryPetStep>>,
    ui: Option<ResMut<crate::ui_script::live::UiEvents>>,
) {
    if let SessionEvent::LoggedOut = ev {
        drop_possess(&mut possess, ui);
        if let Some(mut owed) = owed {
            *owed = Default::default();
        }
    }
}

/// A lost socket sends no zero-guid `SMSG_PET_SPELLS`, so the session end resets the bar itself.
fn on_session_end(
    In(_): In<SessionEvent>,
    mut bar: ResMut<PetBar>,
    mut list: ResMut<super::PetList>,
    mut possess: ResMut<super::possess::PossessSpell>,
    owed: Option<ResMut<super::possess::WorldEntryPetStep>>,
    ui: Option<ResMut<crate::ui_script::live::UiEvents>>,
) {
    bar.clear();
    list.0.clear();
    drop_possess(&mut possess, ui);
    if let Some(mut owed) = owed {
        *owed = Default::default();
    }
}

/// The world leave's possess zero (`0x5d3360`), firing `UPDATE_BONUS_ACTIONBAR` when one was held.
fn drop_possess(
    possess: &mut super::possess::PossessSpell,
    ui: Option<ResMut<crate::ui_script::live::UiEvents>>,
) {
    match ui {
        Some(mut ui) => super::possess::set_possess_spell(possess, 0, &mut ui),
        None => possess.0 = 0,
    }
}

fn on_spells(
    In(ev): In<SessionEvent>,
    catalog: Option<Res<Spells>>,
    mut bar: ResMut<PetBar>,
    mut list: ResMut<super::PetList>,
    mut possess: ResMut<super::possess::PossessSpell>,
    combat: PetCombat,
    player: Query<&crate::net::ObjectStore, With<crate::net::SelfPlayer>>,
    mut ui: ResMut<crate::ui_script::live::UiEvents>,
) {
    if let SessionEvent::PetSpells(spells) = ev {
        let changed = pet_spells(*spells, catalog.as_deref(), &mut bar, &mut list);
        let held = combat.held(bar.spells.pet_guid);
        let dead = player.iter().next().is_some_and(player_dead);
        set_pet_tail(changed, &mut bar, &mut possess, held, dead, &mut ui);
        // The book's re-sort after the setter, `SPELLS_CHANGED` on every packet (`0x5d6d70`, `0x53ceae`).
        ui.fire("SPELLS_CHANGED", Vec::new());
    }
}

/// The held pet's combat test (`0x71afb0`): `UNIT_FIELD_FLAGS` `0x800`, or an auto-attack running.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct PetCombat<'w, 's> {
    index: Option<Res<'w, GuidIndex>>,
    units: Query<
        'w,
        's,
        (
            &'static crate::net::ObjectStore,
            Option<&'static crate::creature_anim::Engaged>,
        ),
    >,
}

/// The held pet as the setter's tail tests it.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct HeldPet {
    /// The combat test (`0x71afb0`).
    pub(crate) in_combat: bool,
    /// It can gain experience (`0x71b630`), a hunter's pet.
    pub(crate) can_gain_xp: bool,
}

impl PetCombat<'_, '_> {
    /// `None` when `guid` names no held unit (`0x5d6a6d`), else its two tests.
    pub(crate) fn held(&self, guid: u64) -> Option<HeldPet> {
        let index = self.index.as_ref()?;
        let &e = index.0.get(&guid)?;
        let (store, engaged) = self.units.get(e).ok()?;
        let creator = store
            .0
            .unit_created_by()
            .and_then(|g| index.0.get(&g))
            .and_then(|&c| self.units.get(c).ok())
            .map(|(s, _)| s);
        store.0.is_unit().then(|| HeldPet {
            in_combat: store.0.unit_flags() & super::unit::UNIT_FLAG_PET_IN_COMBAT != 0
                || engaged.is_some(),
            can_gain_xp: super::temp_bar::can_gain_xp(store, creator),
        })
    }
}

/// Our health at or below 0, the setter's dead test on the active player (`0x5d6b55`).
pub(crate) fn player_dead(store: &crate::net::ObjectStore) -> bool {
    store.0.unit_health().is_some_and(|h| h as i32 <= 0)
}

/// The pet setter's tail (`0x5d6a2f`-`0x5d6b80`): on a new guid the latches drop, a held head's tests run, an empty head zeroes the pet's combo points (`0x5d6ab0`), then `UNIT_PET`; every call updates the temporary bar (`0x5d6ad2`); a new guid while we are `dead` fires `RAISED_AS_GHOUL` for a head (`0x5d6b80`), else refreshes the active player (`0x5d6b6f`); the modal-cast cancel (`0x5d6afb`-`0x5d6b24`) is #1337.
pub(crate) fn set_pet_tail(
    changed: bool,
    bar: &mut PetBar,
    possess: &mut super::possess::PossessSpell,
    held: Option<HeldPet>,
    dead: bool,
    ui: &mut crate::ui_script::live::UiEvents,
) {
    let head = bar.spells.pet_guid != 0;
    if changed {
        // The possess latch and the attacking global drop (`0x5d6a3c`, `0x5d6a42`).
        possess.1 = false;
        bar.attacking = false;
        if !head {
            ui.world(zero_pet_combo);
        } else if let Some(held) = held {
            // The combat test (`0x5d6a80`), then a hunter's pet closes the pet UI (`0x5d6a94`).
            bar.attacking = held.in_combat;
            if held.can_gain_xp {
                ui.fire("PET_UI_CLOSE", Vec::new());
            }
        }
        // `UNIT_PET` with the token pushed ahead (`0x5d6aca`).
        ui.world(super::unit::push_pet_token);
        ui.fire(
            "UNIT_PET",
            vec![benilla_ui::script::ScriptValue::Str("player".into())],
        );
    }
    ui.world(super::temp_bar::update_pet_temp_bar_step);
    if changed && dead {
        if head {
            ui.fire("RAISED_AS_GHOUL", Vec::new());
        } else {
            crate::death::refresh_active_player(0, ui);
        }
    }
}

/// `SetComboPoints(0, 0, 0)` on the setter's empty arm (`0x5d6ab0`): the pet's bank zeroed, which signals as a pet combo packet does (`0x5d34b0`).
fn zero_pet_combo(world: &mut World) {
    if let Some(mut combo) = world.get_resource_mut::<crate::spell::ComboPoints>() {
        combo.pet = Default::default();
        combo.pet_epoch += 1;
    }
}

fn on_mode(
    In(ev): In<SessionEvent>,
    mut bar: ResMut<PetBar>,
    list: Res<super::PetList>,
    mut ui: ResMut<crate::ui_script::live::UiEvents>,
) {
    if let SessionEvent::PetMode(mode) = ev {
        if pet_mode(mode, list.head().unwrap_or(0), &mut bar) {
            // `PET_BAR_UPDATE` whatever changed (`0x5d3689`).
            ui.fire("PET_BAR_UPDATE", Vec::new());
        }
    }
}

fn on_action_feedback(
    In(ev): In<SessionEvent>,
    catalog: Option<Res<Spells>>,
    mut bar: ResMut<PetBar>,
    mut ui: ResMut<crate::ui_script::live::UiEvents>,
) {
    if let SessionEvent::PetActionFeedback { reason, spell } = ev {
        let helpful = catalog
            .as_deref()
            .and_then(|c| c.catalog.get(spell))
            .is_some_and(|d| d.is_helpful());
        debug!("net: pet action feedback {reason}");
        // The line shows in the handler's turn, ahead of the refund (`0x5d4e3c`, `0x5d4e65`).
        if let Some(key) = pet_feedback_key(reason, helpful) {
            crate::ui_action::show_in_turn(&mut ui, "ui_pet", UiError::key(key));
        }
        // A refused target refunds the spell's start cooldown (`0x5d4e42`, `0x5d4e83`).
        if matches!(reason, 2 | 3) {
            pet_refund_start(&mut bar, spell, &mut ui);
        }
    }
}

/// The pet history's start-cooldown refund (`0x804780`): the spellbook's flush when a record matched (`0x80476d`), then the pet bar's (`0x804792`).
fn pet_refund_start(bar: &mut PetBar, spell: u32, ui: &mut crate::ui_script::live::UiEvents) {
    if bar.cooldowns.clear_gcd(spell, Instant::now()) {
        // `UPDATE_SHAPESHIFT_COOLDOWN` between the two while the stance bar holds forms (`0x53bac5`) is #577's.
        ui.fire("ACTIONBAR_UPDATE_COOLDOWN", Vec::new());
        ui.fire("SPELL_UPDATE_COOLDOWN", Vec::new());
    }
    ui.fire("PET_BAR_UPDATE_COOLDOWN", Vec::new());
}

fn on_cast_failed(
    In(ev): In<SessionEvent>,
    mut errors: ResMut<CastErrors>,
    mut bar: ResMut<super::PetBar>,
    mut ends: PetCastEnd,
    mut ui: ResMut<crate::ui_script::live::UiEvents>,
) {
    if let SessionEvent::PetCastFailed {
        cast_count,
        spell_id,
        reason,
        arg,
        wire_reason,
    } = ev
    {
        bar.pending_cast = None;
        // The start cooldown the press armed refunds on the pet history, then the pet bar's flush (`0x806d41`, `0x806d46`).
        pet_refund_start(&mut bar, spell_id, &mut ui);
        pet_cast_failed(spell_id, reason, arg, &mut errors);
        ends.end(spell_id, cast_count, wire_reason);
    }
}

/// What the pet's end-cast reaches (`0x806c30`): the charm, else the summon.
#[derive(bevy::ecs::system::SystemParam)]
struct PetCastEnd<'w, 's> {
    commands: Commands<'w, 's>,
    player: Query<'w, 's, &'static crate::net::ObjectStore, With<crate::net::SelfPlayer>>,
    index: Res<'w, GuidIndex>,
    casting: Query<'w, 's, &'static crate::creature_anim::Casting>,
    spells: Option<Res<'w, Spells>>,
    cast_events: MessageWriter<'w, crate::creature_anim::CastEvent>,
    releases: MessageWriter<'w, crate::creature_anim::MissileRelease>,
    play_seq: ResMut<'w, crate::creature_anim::PlaySeq>,
}

impl PetCastEnd<'_, '_> {
    /// The pet's running cast of this spell and count ends (`0x7fed81`); any other cast's end
    /// still drops a held model of its count (`0x7feeb0`).
    fn end(&mut self, spell_id: u32, cast_count: u8, wire_reason: u8) {
        let pet = self.player.iter().next().and_then(|s| s.0.unit_pet_guid());
        let Some(&entity) = pet.and_then(|g| self.index.0.get(&g)) else {
            return;
        };
        let seq = self.play_seq.next();
        let running = self
            .casting
            .get(entity)
            .is_ok_and(|c| c.ended_by(spell_id, cast_count));
        if running {
            self.commands
                .entity(entity)
                .remove::<crate::creature_anim::Casting>();
            self.cast_events.write(crate::creature_anim::CastEvent {
                entity,
                spell_id,
                kind: crate::creature_anim::CastEventKind::Fail {
                    interrupt: false,
                    wire_reason,
                    ends_cast: true,
                },
                cast_count,
                seq,
            });
        } else if self.spells.as_deref().is_some_and(|s| {
            crate::creature_anim::end_cast_clears_hold(s, spell_id, cast_count, wire_reason)
        }) {
            self.releases.write(crate::creature_anim::MissileRelease {
                caster: entity,
                seq,
                kind: crate::creature_anim::ReleaseKind::Clear(cast_count),
            });
        }
    }
}

fn on_tame_failure(In(ev): In<SessionEvent>, mut failures: ResMut<PetTameFailures>) {
    if let SessionEvent::PetTameFailure { reason } = ev {
        pet_tame_failure(reason, &mut failures);
    }
}

/// `SMSG_PET_NAME_INVALID` (`0x6e2fb1`): the name error (`0x6e2fc0`), then with declined forms `PET_FORCE_NAME_DECLENSION` (`0x6e303d`); `SMSG_PET_BROKEN`'s line.
fn on_refusal_line(In(ev): In<SessionEvent>, mut ui: ResMut<crate::ui_script::live::UiEvents>) {
    use benilla_ui::script::ScriptValue;
    match ev {
        SessionEvent::PetNameInvalid {
            code,
            name,
            declensions: forms,
        } => {
            crate::ui_action::show_in_turn(&mut ui, "ui_pet", pet_name_invalid(code));
            if let Some([a, b, c, d, e]) = forms {
                ui.fire(
                    "PET_FORCE_NAME_DECLENSION",
                    vec![
                        ScriptValue::Str(name),
                        ScriptValue::Str(a),
                        ScriptValue::Str(b),
                        ScriptValue::Str(c),
                        ScriptValue::Str(d),
                        ScriptValue::Str(e),
                    ],
                );
            }
        }
        SessionEvent::PetBroken => {
            debug!("net: pet ran away");
            crate::ui_action::show_in_turn(&mut ui, "ui_pet", UiError::key("ERR_PET_BROKEN"));
        }
        _ => {}
    }
}

fn on_action_sound(
    In(ev): In<SessionEvent>,
    index: Res<GuidIndex>,
    mut talks: MessageWriter<PetTalkMessage>,
) {
    if let SessionEvent::PetActionSound { pet_guid, talk } = ev {
        pet_action_sound(pet_guid, talk, &index, &mut talks);
    }
}

fn on_dismiss_sound(In(ev): In<SessionEvent>, mut sounds: MessageWriter<PetDismissSoundMessage>) {
    if let SessionEvent::PetDismissSound { model_id, position } = ev {
        pet_dismiss_sound(model_id, position, &mut sounds);
    }
}

fn on_spell_line(
    In(ev): In<SessionEvent>,
    catalog: Option<Res<Spells>>,
    mut ui: ResMut<crate::ui_script::live::UiEvents>,
) {
    let line = match ev {
        SessionEvent::PetLearnedSpell { spell_id } => {
            pet_spell_line(spell_id, true, catalog.as_deref())
        }
        SessionEvent::PetUnlearnedSpell { spell_id } => {
            pet_spell_line(spell_id, false, catalog.as_deref())
        }
        _ => None,
    };
    if let Some(line) = line {
        crate::ui_action::show_in_turn(&mut ui, "ui_pet", line);
    }
}

/// `SMSG_PET_RENAMEABLE` (`0x5d3140`): `PET_RENAMEABLE`, which the stock `PetFrame` answers with the `RENAME_PET` popup (`0x5d3147`).
fn on_renameable(In(ev): In<SessionEvent>, mut ui: ResMut<crate::ui_script::live::UiEvents>) {
    if let SessionEvent::PetRenameable = ev {
        debug!("net: pet renameable");
        ui.fire("PET_RENAMEABLE", Vec::new());
    }
}

fn on_guids(In(ev): In<SessionEvent>, mut bar: ResMut<PetBar>, mut list: ResMut<super::PetList>) {
    if let SessionEvent::PetGuids(guids) = ev {
        pet_guids(guids, &mut bar, &mut list);
    }
}

/// `SMSG_PET_SPELLS`: replace the whole bar and reseed the pet's cooldowns from its tail. A zero
/// guid is the teardown (`Player::RemovePetActionBar`), cooldowns included.
fn pet_spells(
    spells: PetSpells,
    catalog: Option<&Spells>,
    bar: &mut PetBar,
    list: &mut super::PetList,
) -> bool {
    // The setter compares the guid it caches, then caches the new one (`0x5d690f`, `0x5d6932`).
    let changed = std::mem::replace(&mut bar.cached, spells.pet_guid) != spells.pet_guid;
    if spells.pet_guid == 0 {
        if bar.spells.pet_guid != 0 {
            debug!("net: pet bar torn down");
        }
        // The words, the pet book and the pet's cooldowns empty first (`0x5d6ba0`, `0x5d6be1`), then the setter empties the list and the expiry (`0x5d6943`, `0x5d6a29`).
        bar.spells = PetSpells::default();
        bar.cooldowns.clear_silent();
        bar.expires = None;
        list.0.clear();
        return changed;
    }
    // The pet setter (`0x5d6900`): a listed guid moves to the head, swapping places with it; an
    // unlisted one becomes the whole list.
    match list.0.iter().position(|&g| g == spells.pet_guid) {
        Some(i) => list.0.swap(0, i),
        None => list.0 = vec![spells.pet_guid],
    }
    debug!(
        "net: pet bar for {:#x} — react {} command {} {}, {} known spell(s), {} cooldown(s)",
        spells.pet_guid,
        spells.react_state(),
        spells.command_state(),
        if spells.vehicle() {
            "VEHICLE"
        } else {
            "usable"
        },
        spells.spells.len(),
        spells.cooldowns.len(),
    );
    debug!(
        "net: pet bar slots — {}",
        spells
            .bar
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let what = match e.kind() {
                    benilla_protocol::messages::PET_ACT_COMMAND => format!("cmd {}", e.action()),
                    benilla_protocol::messages::PET_ACT_REACTION => format!("react {}", e.action()),
                    _ if e.is_empty() => "empty".to_string(),
                    _ => match catalog.and_then(|c| c.catalog.get(e.action())) {
                        Some(d) => format!("{} ({})", d.name, e.action()),
                        None => format!("spell {} NOT IN CATALOG", e.action()),
                    },
                };
                format!("{}:{what}/{:#04x}", i + 1, e.kind())
            })
            .collect::<Vec<_>>()
            .join(" ")
    );
    // vmangos sends the pet's passives here too; the book skips `DO_NOT_DISPLAY`.
    debug!(
        "net: pet spellbook — {}",
        spells
            .spells
            .iter()
            .map(|e| {
                match catalog.and_then(|c| c.catalog.get(e.action())) {
                    Some(d) if d.in_pet_book() => format!("{} ({})", d.name, e.action()),
                    Some(d) => format!("{} ({}) DO_NOT_DISPLAY", d.name, e.action()),
                    None => format!("{} NOT IN CATALOG", e.action()),
                }
            })
            .collect::<Vec<_>>()
            .join(" · ")
    );
    let now = Instant::now();
    // The pet setter re-arms the expiry on every packet, and a 0 duration clears it.
    bar.expires = (spells.duration_ms != 0)
        .then(|| now + std::time::Duration::from_millis(u64::from(spells.duration_ms)));
    bar.cooldowns.clear_silent();
    for cd in &spells.cooldowns {
        let display = catalog.and_then(|c| c.catalog.get(cd.spell_id));
        bar.cooldowns.seed_pet(cd, display, now);
    }
    bar.spells = spells;
    changed
}

/// `SMSG_PET_GUIDS` (`0x5d6550`): the list replaced whole, with no event. Every pet verb reads the
/// list's head (`0x5d3390` with index 0), so the head is the bar's guid; an empty list is no pet
/// while the bar's words stay for the next `SMSG_PET_SPELLS`.
fn pet_guids(guids: Vec<u64>, bar: &mut PetBar, list: &mut super::PetList) {
    debug!("net: pet guids {guids:x?}");
    bar.spells.pet_guid = guids.first().copied().unwrap_or(0);
    list.0 = guids;
}

/// `Attributes & 0x10`, which picks `ERR_PET_LEARN_ABILITY_S` over the spell line (`0x5d4d62`).
const ATTR_ABILITY: u32 = 0x10;

/// `SMSG_PET_LEARNED_SPELL` / `SMSG_PET_UNLEARNED_SPELL` (`0x5d4c30`): one system chat line with
/// the name and rank, `"%s (%s)"`; an id `Spell.dbc` lacks says nothing.
fn pet_spell_line(spell_id: u32, learned: bool, catalog: Option<&Spells>) -> Option<UiError> {
    let display = catalog.and_then(|c| c.catalog.get(spell_id))?;
    let key = match (learned, display.attributes & ATTR_ABILITY != 0) {
        (false, _) => "ERR_PET_SPELL_UNLEARNED_S",
        (true, true) => "ERR_PET_LEARN_ABILITY_S",
        (true, false) => "ERR_PET_LEARN_SPELL_S",
    };
    debug!("net: pet spell {spell_id} — {key}");
    Some(UiError::s(key, display.ranked_name()))
}

/// `SMSG_PET_MODE` (`0x5d3630`): the state word alone, applied when the packet's guid is the pet list's `head`, 0 for an empty list (`0x5d366e`, `0x5d3683`); whether it applied.
fn pet_mode(mode: PetMode, head: u64, bar: &mut PetBar) -> bool {
    if mode.pet_guid != head {
        return false;
    }
    debug!("net: pet mode — state {:#010x}", mode.state);
    // The whole dword, as the client stores both packets' state.
    bar.spells.state = mode.state;
    true
}

/// The reference's switch (`0x5d4dc8`): 1 `ERR_PET_SPELL_DEAD` (`0x5d4dcf`), 2
/// `ERR_NO_ATTACK_TARGET` or, for a helpful spell, `ERR_GENERIC_NO_VALID_TARGETS` (`0x5d4e29`,
/// `0x5d4e36`), 3 `ERR_INVALID_ATTACK_TARGET` (`0x5d4e60`), 4 `ERR_PET_SPELL_NOPATH`
/// (`0x5d4e95`); other bytes show nothing.
fn pet_feedback_key(reason: u8, helpful: bool) -> Option<&'static str> {
    Some(match reason {
        1 => "ERR_PET_SPELL_DEAD",
        2 if helpful => "ERR_GENERIC_NO_VALID_TARGETS",
        2 => "ERR_NO_ATTACK_TARGET",
        3 => "ERR_INVALID_ATTACK_TARGET",
        4 => "ERR_PET_SPELL_NOPATH",
        _ => return None,
    })
}

/// `SMSG_PET_TAME_FAILURE`: the reason byte, queued. vmangos also sends it for a summon while a
/// pet or a charmed unit is out (`Spell.cpp:5467`).
fn pet_tame_failure(reason: u8, failures: &mut PetTameFailures) {
    debug!(
        "net: pet tame failure {reason} ({})",
        benilla_protocol::messages::pet_tame_failure_key(reason)
    );
    failures.0.push(reason);
}

/// `SMSG_PET_NAME_INVALID`: the server's name code through the name-error display, as the client's
/// own check would show it. No local name was applied, so nothing rolls back.
fn pet_name_invalid(code: u32) -> UiError {
    let key = NameError(code).key();
    debug!("net: pet name refused, code {code} ({key})");
    UiError::key(key)
}

/// `SMSG_PET_ACTION_SOUND`: dropped when the pet is not streamed; the sound layer checks the
/// selector ([`crate::sound::creature`]).
fn pet_action_sound(
    pet_guid: u64,
    talk: u32,
    index: &GuidIndex,
    talks: &mut MessageWriter<PetTalkMessage>,
) {
    debug!("net: pet talk {talk} from {pet_guid:#x}");
    if let Some(&unit) = index.0.get(&pet_guid) {
        talks.write(PetTalkMessage { unit, talk });
    }
}

/// `SMSG_PET_DISMISS_SOUND`: sounded a yard above the wire point (`fadd`), in WoW space, so before
/// the change to Bevy's.
fn pet_dismiss_sound(
    model_id: u32,
    position: [f32; 3],
    sounds: &mut MessageWriter<PetDismissSoundMessage>,
) {
    debug!("net: pet dismissed — model {model_id} at {position:?}");
    sounds.write(PetDismissSoundMessage {
        model_id,
        pos: wow_to_bevy([position[0], position[1], position[2] + 1.0]),
    });
}

/// `SMSG_PET_CAST_FAILED`: queued as the pet's, so the display uses the reference's pet table. Our
/// cast state is untouched, and unlike the player's it writes no combat-log line and plays no error
/// sound.
fn pet_cast_failed(spell_id: u32, reason: u8, arg: Option<u32>, errors: &mut CastErrors) {
    debug!("net: pet cast failed — spell {spell_id} reason {reason:#04x} arg {arg:?}");
    errors.push_pet(spell_id, reason, arg);
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::messages::{PetActionEntry, PET_ACT_COMMAND, PET_COMMAND_ATTACK};

    fn a_bar(guid: u64) -> PetSpells {
        let mut s = PetSpells {
            pet_guid: guid,
            state: 0x0101, // react Defensive, command Follow
            ..Default::default()
        };
        s.bar[0] = PetActionEntry::from(PET_COMMAND_ATTACK | (u32::from(PET_ACT_COMMAND) << 24));
        s
    }

    /// The server's name code shows the client's own name error.
    #[test]
    fn a_refused_rename_shows_its_name_error() {
        let keys = [
            pet_name_invalid(NameError::TOO_LONG.0).key,
            pet_name_invalid(99).key,
        ];
        assert_eq!(keys, ["ERR_NAME_TOO_LONG", "ERR_NAME_INVALID"]);
    }

    #[test]
    fn a_zero_guid_tears_the_whole_bar_down() {
        let mut bar = PetBar::default();
        let mut list = super::super::PetList::default();
        pet_spells(a_bar(0x2A), None, &mut bar, &mut list);
        assert_eq!(bar.spells.pet_guid, 0x2A);

        pet_spells(PetSpells::default(), None, &mut bar, &mut list);
        assert_eq!(bar.spells, PetSpells::default());
        assert!(!bar.has_bar());
    }

    /// The pet setter: each packet re-arms the expiry from its own duration, 0 clears it, and the
    /// teardown clears it too.
    #[test]
    fn each_pet_packet_rearms_or_clears_the_expiry() {
        let mut bar = PetBar::default();
        let mut list = super::super::PetList::default();
        let mut charm = a_bar(0x2A);
        charm.duration_ms = 60_000;
        let before = Instant::now();
        pet_spells(charm.clone(), None, &mut bar, &mut list);
        let left = bar.expires.expect("armed").duration_since(before);
        assert!(
            left >= std::time::Duration::from_secs(60) && left < std::time::Duration::from_secs(61)
        );

        pet_spells(a_bar(0x2A), None, &mut bar, &mut list);
        assert_eq!(bar.expires, None, "a 0 duration clears it");
        pet_spells(charm, None, &mut bar, &mut list);
        assert!(bar.expires.is_some());
        pet_spells(PetSpells::default(), None, &mut bar, &mut list);
        assert_eq!(bar.expires, None, "the teardown clears it");
    }

    #[test]
    fn pet_mode_only_writes_its_own_bar() {
        let mut bar = PetBar::default();
        let mut list = super::super::PetList::default();
        pet_spells(a_bar(0x2A), None, &mut bar, &mut list);

        pet_mode(
            PetMode {
                pet_guid: 0x99,
                state: 0x0202,
            },
            list.head().unwrap_or(0),
            &mut bar,
        );
        assert_eq!(bar.spells.react_state(), 1, "a stranger's mode is ignored");

        pet_mode(
            PetMode {
                pet_guid: 0x2A,
                state: 0x0202,
            },
            list.head().unwrap_or(0),
            &mut bar,
        );
        assert_eq!(bar.spells.react_state(), 2);
        assert_eq!(bar.spells.command_state(), 2);
        assert_eq!(bar.spells.bar[0].action(), PET_COMMAND_ATTACK);
    }

    /// The setter's tail writes the attacking global only on a new guid (`0x5d6a42`), then from the new pet's combat test (`0x5d6a80`); every packet runs the temporary bar's update.
    #[test]
    fn the_attack_latch_survives_a_resend_and_dies_on_a_new_pet() {
        let mut bar = PetBar::default();
        let mut list = super::super::PetList::default();
        let mut possess = super::super::possess::PossessSpell::default();
        let mut ui = crate::ui_script::live::UiEvents::default();
        let mut set = |bar: &mut PetBar, guid, in_combat: Option<bool>| {
            let changed = pet_spells(a_bar(guid), None, bar, &mut list);
            let held = in_combat.map(|in_combat| HeldPet {
                in_combat,
                can_gain_xp: false,
            });
            set_pet_tail(changed, bar, &mut possess, held, false, &mut ui);
        };
        set(&mut bar, 0x2A, None);
        bar.attacking = true;

        set(&mut bar, 0x2A, Some(false));
        assert!(bar.attacking, "the same pet re-sending keeps its attack");

        set(&mut bar, 0x99, None);
        assert!(!bar.attacking, "a different pet is not attacking");
        set(&mut bar, 0x2A, Some(true));
        assert!(bar.attacking, "a new pet in combat starts attacking");
    }

    fn catalog() -> Spells {
        let mut spells = Spells::empty_for_tests();
        let row = |name: &str, rank: &str, attributes| benilla_formats::SpellDisplay {
            name: name.to_string(),
            rank: Some(rank.to_string()),
            attributes,
            ..Default::default()
        };
        spells.catalog = benilla_formats::SpellCatalog::from_displays(
            [
                (17253, row("Bite", "Rank 1", 0x10)),
                (24450, row("Prowl", "", 0)),
            ]
            .into_iter()
            .collect(),
        );
        spells
    }

    /// `0x5d4c30`: `Attributes & 0x10` picks the ability line, the rank rides in parentheses, the
    /// unlearn has one line for both, and an id the table lacks says nothing.
    #[test]
    fn a_pet_learn_or_unlearn_prints_its_system_line() {
        let spells = catalog();
        let errors: Vec<UiError> = [
            pet_spell_line(17253, true, Some(&spells)),
            pet_spell_line(24450, true, Some(&spells)),
            pet_spell_line(17253, false, Some(&spells)),
            pet_spell_line(99999, true, Some(&spells)),
            pet_spell_line(17253, true, None),
        ]
        .into_iter()
        .flatten()
        .collect();
        let lines: Vec<(&str, Option<&str>)> = errors.iter().map(|e| (e.key, e.arg_s())).collect();
        assert_eq!(
            lines,
            [
                ("ERR_PET_LEARN_ABILITY_S", Some("Bite (Rank 1)")),
                ("ERR_PET_LEARN_SPELL_S", Some("Prowl")),
                ("ERR_PET_SPELL_UNLEARNED_S", Some("Bite (Rank 1)")),
            ]
        );
        for (key, _) in lines {
            let row = benilla_ui::messages::by_key(key).expect("a catalog row");
            assert_eq!(row.kind, benilla_ui::messages::MsgKind::Chat, "{key}");
        }
    }

    /// The pet setter moves a listed guid to the head and replaces the list with an unlisted one;
    /// `SMSG_PET_GUIDS` replaces it whole, its head the bar's guid.
    #[test]
    fn the_pet_list_follows_the_setter_and_the_guid_packet() {
        let mut bar = PetBar::default();
        let mut list = super::super::PetList::default();
        pet_spells(a_bar(0x2A), None, &mut bar, &mut list);
        assert_eq!(list.0, [0x2A]);

        pet_guids(vec![0x2A, 0x99, 0x77], &mut bar, &mut list);
        assert_eq!(bar.spells.pet_guid, 0x2A);
        pet_spells(a_bar(0x77), None, &mut bar, &mut list);
        assert_eq!(list.0, [0x77, 0x99, 0x2A], "a swap with the head");
        pet_spells(a_bar(0x55), None, &mut bar, &mut list);
        assert_eq!(list.0, [0x55]);

        pet_guids(vec![0x99], &mut bar, &mut list);
        assert_eq!(bar.spells.pet_guid, 0x99, "the head is the pet");
        pet_guids(Vec::new(), &mut bar, &mut list);
        assert!(!bar.has_bar(), "an empty list is no pet");
        assert!(!bar.spells.bar[0].is_empty(), "the words stay");

        pet_spells(PetSpells::default(), None, &mut bar, &mut list);
        assert!(list.0.is_empty());
    }

    /// The setter's change test reads its own cached guid, which `SMSG_PET_GUIDS` never writes (`0x5d6903`, `0x5d6550`).
    #[test]
    fn the_setters_change_test_ignores_the_guid_list() {
        let mut bar = PetBar::default();
        let mut list = super::super::PetList::default();
        assert!(pet_spells(a_bar(0x2A), None, &mut bar, &mut list));
        pet_guids(vec![0x99], &mut bar, &mut list);
        assert!(
            pet_spells(a_bar(0x99), None, &mut bar, &mut list),
            "the cache still holds the old pet"
        );
        pet_guids(Vec::new(), &mut bar, &mut list);
        assert!(
            !pet_spells(a_bar(0x99), None, &mut bar, &mut list),
            "an emptied list leaves the cache"
        );
        assert!(pet_spells(PetSpells::default(), None, &mut bar, &mut list));
        assert_eq!(bar.cached, 0);
    }

    #[test]
    fn the_feedback_map_is_the_switch_at_0x5d4dc8() {
        let mut keys: Vec<&str> = [0u8, 1, 2, 3, 4, 5, 200]
            .into_iter()
            .filter_map(|reason| pet_feedback_key(reason, false))
            .collect();
        // A helpful spell's refusal reads as no valid targets; reason 3 ignores the spell.
        keys.extend(pet_feedback_key(2, true));
        keys.extend(pet_feedback_key(3, true));
        assert_eq!(
            keys,
            [
                "ERR_PET_SPELL_DEAD",
                "ERR_NO_ATTACK_TARGET",
                "ERR_INVALID_ATTACK_TARGET",
                "ERR_PET_SPELL_NOPATH",
                "ERR_GENERIC_NO_VALID_TARGETS",
                "ERR_INVALID_ATTACK_TARGET",
            ]
        );
    }

    /// `SMSG_PET_MODE` signals `PET_BAR_UPDATE` on every packet for the bar's pet (`0x5d3689`).
    #[test]
    fn a_repeated_pet_mode_still_signals_the_bar() {
        let mut bar = PetBar::default();
        bar.spells.pet_guid = 0xF14;
        let mode = |pet_guid| PetMode {
            pet_guid,
            state: 0x0100,
        };
        assert!(pet_mode(mode(0xF14), 0xF14, &mut bar));
        assert!(
            pet_mode(mode(0xF14), 0xF14, &mut bar),
            "the same state signals again"
        );
        assert!(
            !pet_mode(mode(0xABC), 0xF14, &mut bar),
            "another pet's mode is not ours"
        );
    }

    /// The mode handler compares the packet's guid with the list's head, 0 for an empty list, so a zero-guid mode with no pet still writes the word and signals (`0x5d366e`, `0x5d3683`, `0x5d3689`).
    #[test]
    fn a_zero_guid_mode_with_no_pet_writes_and_signals() {
        let mut bar = PetBar::default();
        let mode = |pet_guid| PetMode {
            pet_guid,
            state: 0x0201,
        };
        assert!(
            !pet_mode(mode(0xF14), 0, &mut bar),
            "no head, no pet's mode"
        );
        assert_eq!(bar.spells.state, 0);
        assert!(pet_mode(mode(0), 0, &mut bar));
        assert_eq!(bar.spells.state, 0x0201);
    }

    /// The zero-guid packet empties the words, the book and the cooldowns, and the setter's change test alone drops the attacking global (`0x5d6ba0`, `0x5d6be1`, `0x5d6a42`); a repeated teardown keeps a global it did not change.
    #[test]
    fn a_teardown_drops_the_attack_only_on_a_change() {
        let mut bar = PetBar::default();
        let mut list = super::super::PetList::default();
        let mut possess = super::super::possess::PossessSpell::default();
        let mut ui = crate::ui_script::live::UiEvents::default();
        let changed = pet_spells(a_bar(0x2A), None, &mut bar, &mut list);
        set_pet_tail(changed, &mut bar, &mut possess, None, false, &mut ui);
        bar.attacking = true;
        let changed = pet_spells(PetSpells::default(), None, &mut bar, &mut list);
        set_pet_tail(changed, &mut bar, &mut possess, None, false, &mut ui);
        assert!(!bar.attacking);
        assert_eq!(bar.spells, PetSpells::default());
        assert!(list.0.is_empty());
        bar.attacking = true;
        let changed = pet_spells(PetSpells::default(), None, &mut bar, &mut list);
        assert!(!changed);
        set_pet_tail(changed, &mut bar, &mut possess, None, false, &mut ui);
        assert!(bar.attacking, "no change, no drop");
    }

    /// The setter's dead arm on a new guid (`0x5d6b55`): a head fires `RAISED_AS_GHOUL` after the temporary bar (`0x5d6b80`), no head refreshes the active player, `PLAYER_DEAD` with a cursor clear (`0x5d6b6f`); the empty arm zeroes the pet's combo points first (`0x5d6ab0`); alive, neither fires.
    #[test]
    fn a_new_pet_while_dead_raises_the_ghoul() {
        let mut bar = PetBar::default();
        let mut list = super::super::PetList::default();
        let mut possess = super::super::possess::PossessSpell::default();
        let mut ui = crate::ui_script::live::UiEvents::default();
        let changed = pet_spells(a_bar(0x2A), None, &mut bar, &mut list);
        set_pet_tail(changed, &mut bar, &mut possess, None, true, &mut ui);
        assert_eq!(
            ui.names(),
            ["<world>", "UNIT_PET", "<world>", "RAISED_AS_GHOUL"]
        );
        let mut ui = crate::ui_script::live::UiEvents::default();
        let changed = pet_spells(PetSpells::default(), None, &mut bar, &mut list);
        set_pet_tail(changed, &mut bar, &mut possess, None, true, &mut ui);
        assert_eq!(
            ui.names(),
            [
                "<world>",
                "<world>",
                "UNIT_PET",
                "<world>",
                "PLAYER_DEAD",
                "<clear cursor>"
            ]
        );
        let mut ui = crate::ui_script::live::UiEvents::default();
        let changed = pet_spells(a_bar(0x2B), None, &mut bar, &mut list);
        set_pet_tail(changed, &mut bar, &mut possess, None, false, &mut ui);
        assert_eq!(ui.names(), ["<world>", "UNIT_PET", "<world>"]);
    }

    #[test]
    fn every_feedback_arm_is_a_catalog_row() {
        for (reason, helpful) in [(1u8, false), (2, false), (2, true), (3, false), (4, false)] {
            let key = pet_feedback_key(reason, helpful).expect("an arm");
            let row = benilla_ui::messages::by_key(key)
                .unwrap_or_else(|| panic!("{key} is not a message-catalog row"));
            assert_eq!(
                row.kind,
                benilla_ui::messages::MsgKind::Error,
                "{key} is the red line"
            );
        }
    }

    /// The pet's running cast of this spell and count ends through the cast edge; any other drops
    /// only a numbered flag-1 missile's held model of its count (`0x7fed81`, `0x7feeb0`).
    #[test]
    fn the_pet_s_cast_failure_ends_its_cast_or_drops_its_hold() {
        use crate::creature_anim::{
            CastEvent, CastEventKind, Casting, MissileRelease, ReleaseKind,
        };
        use benilla_formats::{SpellCatalog, SpellDisplay};
        use bevy::ecs::system::RunSystemOnce;
        let mut app = App::new();
        app.add_message::<CastEvent>()
            .add_message::<MissileRelease>()
            .init_resource::<crate::creature_anim::PlaySeq>();
        let aim = SpellDisplay {
            flies_aim: true,
            ..Default::default()
        };
        app.insert_resource(Spells {
            catalog: SpellCatalog::from_displays(
                [(7, aim), (8, SpellDisplay::default())]
                    .into_iter()
                    .collect(),
            ),
            ..Spells::empty_for_tests()
        });
        let pet = app
            .world_mut()
            .spawn(Casting {
                spell_id: 7,
                cast_count: 3,
                until: None,
                listed: true,
                hidden: false,
            })
            .id();
        app.insert_resource(GuidIndex([(0x2A, pet)].into_iter().collect()));
        app.world_mut().spawn((
            crate::net::SelfPlayer,
            crate::net::ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[(
                benilla_protocol::field::FIELD_UNIT_SUMMON,
                0x2A,
            )])),
        ));
        let end = |app: &mut App, spell_id: u32, cast_count: u8, wire_reason: u8| {
            app.world_mut()
                .run_system_once(move |mut ends: PetCastEnd| {
                    ends.end(spell_id, cast_count, wire_reason);
                })
                .expect("run_system_once");
            let casts: Vec<(u32, u8)> = app
                .world_mut()
                .resource_mut::<Messages<CastEvent>>()
                .drain()
                .map(|e| {
                    assert!(matches!(
                        e.kind,
                        CastEventKind::Fail {
                            interrupt: false,
                            ..
                        }
                    ));
                    (e.spell_id, e.cast_count)
                })
                .collect();
            let releases: Vec<ReleaseKind> = app
                .world_mut()
                .resource_mut::<Messages<MissileRelease>>()
                .drain()
                .map(|r| r.kind)
                .collect();
            (casts, releases)
        };
        assert_eq!(
            end(&mut app, 7, 4, 0x0c),
            (vec![], vec![ReleaseKind::Clear(4)])
        );
        assert_eq!(
            end(&mut app, 7, 0, 0x0c),
            (vec![], vec![]),
            "an unnumbered cast"
        );
        assert_eq!(
            end(&mut app, 7, 4, 187),
            (vec![], vec![]),
            "the launch result"
        );
        assert_eq!(
            end(&mut app, 7, 4, 188),
            (vec![], vec![ReleaseKind::Clear(4)]),
            "a reason past it, shared 0x91 with it"
        );
        assert_eq!(
            end(&mut app, 8, 4, 0x0c),
            (vec![], vec![]),
            "no flag-1 missile"
        );
        assert!(app.world().get::<Casting>(pet).is_some());
        assert_eq!(end(&mut app, 7, 3, 0x0c), (vec![(7, 3)], vec![]));
        assert!(
            app.world().get::<Casting>(pet).is_none(),
            "the running cast ends"
        );
    }
}
