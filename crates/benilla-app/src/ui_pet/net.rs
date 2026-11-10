//! The pet's packet handlers: `SMSG_PET_SPELLS` and `SMSG_PET_MODE` write [`PetBar`]; the refusals,
//! the tame failure and the pet's sounds go to their queues. The pet's refusals share the player's
//! red line but not its message tables: the reference maps them in their own handlers to say "your
//! pet" where the player's say "you".

use std::time::Instant;

use bevy::prelude::*;

use benilla_protocol::messages::{PetMode, PetSpells};
use benilla_protocol::{SessionEvent, SessionEventKind};
use benilla_ui::names::NameError;

use benilla_assets::coords::wow_to_bevy;

use super::PetBar;
use crate::net::{GuidIndex, NetHandlerApp, PetDismissSoundMessage, PetTalkMessage};
use crate::ui_action::{CastErrors, PetTameFailures, Spells, UiError, UiErrorKeys};

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
        .net_handler(K::Disconnected, on_session_end);
}

/// A lost socket sends no zero-guid `SMSG_PET_SPELLS`, so the session end resets the bar itself.
fn on_session_end(In(_): In<SessionEvent>, mut bar: ResMut<PetBar>) {
    bar.clear();
}

fn on_spells(In(ev): In<SessionEvent>, catalog: Option<Res<Spells>>, mut bar: ResMut<PetBar>) {
    if let SessionEvent::PetSpells(spells) = ev {
        pet_spells(*spells, catalog.as_deref(), &mut bar);
    }
}

fn on_mode(In(ev): In<SessionEvent>, mut bar: ResMut<PetBar>) {
    if let SessionEvent::PetMode(mode) = ev {
        pet_mode(mode, &mut bar);
    }
}

fn on_action_feedback(
    In(ev): In<SessionEvent>,
    mut errors: ResMut<UiErrorKeys>,
    catalog: Option<Res<Spells>>,
    mut bar: ResMut<PetBar>,
) {
    if let SessionEvent::PetActionFeedback { reason, spell } = ev {
        let helpful = catalog
            .as_deref()
            .and_then(|c| c.catalog.get(spell))
            .is_some_and(|d| d.is_helpful());
        pet_action_feedback(reason, helpful, &mut errors);
        // A refused target refunds the spell's start cooldown and refreshes the pet's
        // (`0x5d4e42`, `0x5d4e83` -> `0x804780`).
        if matches!(reason, 2 | 3) {
            bar.cooldowns.clear_gcd(spell, std::time::Instant::now());
        }
    }
}

fn on_cast_failed(
    In(ev): In<SessionEvent>,
    mut errors: ResMut<CastErrors>,
    mut bar: ResMut<super::PetBar>,
    mut ends: PetCastEnd,
) {
    if let SessionEvent::PetCastFailed {
        cast_count,
        spell_id,
        reason,
        arg,
    } = ev
    {
        bar.pending_cast = None;
        // The start cooldown the press armed clears, then the pet bar refreshes (`0x806d41`).
        bar.cooldowns.clear_gcd(spell_id, std::time::Instant::now());
        pet_cast_failed(spell_id, reason, arg, &mut errors);
        ends.end(spell_id, cast_count, reason);
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
    fn end(&mut self, spell_id: u32, cast_count: u8, reason: u8) {
        let pet = self.player.iter().next().and_then(|s| s.0.unit_pet_guid());
        let Some(&entity) = pet.and_then(|g| self.index.0.get(&g)) else {
            return;
        };
        let seq = self.play_seq.next();
        let running = self
            .casting
            .get(entity)
            .is_ok_and(|c| c.spell_id == spell_id && c.cast_count == cast_count);
        if running {
            self.commands
                .entity(entity)
                .remove::<crate::creature_anim::Casting>();
            self.cast_events.write(crate::creature_anim::CastEvent {
                entity,
                spell_id,
                kind: crate::creature_anim::CastEventKind::Fail {
                    interrupt: false,
                    reason,
                },
                cast_count,
                seq,
            });
        } else if self.spells.as_deref().is_some_and(|s| {
            crate::creature_anim::end_cast_clears_hold(s, spell_id, cast_count, reason)
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

fn on_refusal_line(
    In(ev): In<SessionEvent>,
    mut errors: ResMut<UiErrorKeys>,
    mut declensions: ResMut<super::PetDeclensions>,
) {
    match ev {
        SessionEvent::PetNameInvalid {
            code,
            name,
            declensions: forms,
        } => {
            pet_name_invalid(code, &mut errors);
            if let Some(forms) = forms {
                declensions.0.push((name, forms));
            }
        }
        SessionEvent::PetBroken => pet_broken(&mut errors),
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
    mut errors: ResMut<UiErrorKeys>,
) {
    match ev {
        SessionEvent::PetLearnedSpell { spell_id } => {
            pet_spell_line(spell_id, true, catalog.as_deref(), &mut errors)
        }
        SessionEvent::PetUnlearnedSpell { spell_id } => {
            pet_spell_line(spell_id, false, catalog.as_deref(), &mut errors)
        }
        _ => {}
    }
}

/// `SMSG_PET_RENAMEABLE`: counted for the menu drain, which fires `PET_RENAMEABLE`.
fn on_renameable(In(ev): In<SessionEvent>, mut renameable: ResMut<super::PetRenameable>) {
    if let SessionEvent::PetRenameable = ev {
        debug!("net: pet renameable");
        renameable.0 += 1;
    }
}

fn on_guids(In(ev): In<SessionEvent>, mut bar: ResMut<PetBar>) {
    if let SessionEvent::PetGuids(guids) = ev {
        pet_guids(guids, &mut bar);
    }
}

/// `SMSG_PET_SPELLS`: replace the whole bar and reseed the pet's cooldowns from its tail. A zero
/// guid is the teardown (`Player::RemovePetActionBar`), cooldowns included.
fn pet_spells(spells: PetSpells, catalog: Option<&Spells>, bar: &mut PetBar) {
    if spells.pet_guid == 0 {
        if bar.spells.pet_guid != 0 {
            debug!("net: pet bar torn down");
        }
        bar.clear();
        return;
    }
    // A new pet guid clears the attack latch; a re-send from the same pet keeps it.
    if bar.spells.pet_guid != spells.pet_guid {
        bar.attacking = false;
    }
    // The pet setter (`0x5d6900`): a listed guid moves to the head, swapping places with it; an
    // unlisted one becomes the whole list.
    match bar.pets.iter().position(|&g| g == spells.pet_guid) {
        Some(i) => bar.pets.swap(0, i),
        None => bar.pets = vec![spells.pet_guid],
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
    // The pet setter ends in a `PET_BAR_UPDATE` signal (event `0x161`) on every packet, whatever
    // changed: the one repaint a packet that moves only the pet's cooldowns gets.
    bar.bar_signals = bar.bar_signals.wrapping_add(1);
}

/// `SMSG_PET_GUIDS` (`0x5d6550`): the list replaced whole, with no event. Every pet verb reads the
/// list's head (`0x5d3390` with index 0), so the head is the bar's guid; an empty list is no pet
/// while the bar's words stay for the next `SMSG_PET_SPELLS`.
fn pet_guids(guids: Vec<u64>, bar: &mut PetBar) {
    debug!("net: pet guids {guids:x?}");
    bar.spells.pet_guid = guids.first().copied().unwrap_or(0);
    bar.pets = guids;
}

/// `Attributes & 0x10`, which picks `ERR_PET_LEARN_ABILITY_S` over the spell line (`0x5d4d62`).
const ATTR_ABILITY: u32 = 0x10;

/// `SMSG_PET_LEARNED_SPELL` / `SMSG_PET_UNLEARNED_SPELL` (`0x5d4c30`): one system chat line with
/// the name and rank, `"%s (%s)"`; an id `Spell.dbc` lacks says nothing.
fn pet_spell_line(
    spell_id: u32,
    learned: bool,
    catalog: Option<&Spells>,
    errors: &mut UiErrorKeys,
) {
    let Some(display) = catalog.and_then(|c| c.catalog.get(spell_id)) else {
        return;
    };
    let key = match (learned, display.attributes & ATTR_ABILITY != 0) {
        (false, _) => "ERR_PET_SPELL_UNLEARNED_S",
        (true, true) => "ERR_PET_LEARN_ABILITY_S",
        (true, false) => "ERR_PET_LEARN_SPELL_S",
    };
    debug!("net: pet spell {spell_id} — {key}");
    errors.0.push(UiError::s(key, display.ranked_name()));
}

/// `SMSG_PET_MODE`: the state word alone, applied only to the bar of the pet it names.
fn pet_mode(mode: PetMode, bar: &mut PetBar) {
    if bar.spells.pet_guid == 0 || bar.spells.pet_guid != mode.pet_guid {
        return;
    }
    debug!("net: pet mode — state {:#010x}", mode.state);
    // The whole dword, as the client stores both packets' state, and `PET_BAR_UPDATE` whatever
    // changed (`0x5d3689`).
    bar.spells.state = mode.state;
    bar.bar_signals = bar.bar_signals.wrapping_add(1);
}

/// `SMSG_PET_ACTION_FEEDBACK`: a refused order's reason byte, onto the red line; `helpful` is
/// the refused spell's helpful-or-harmful answer, which reason 2 reads.
fn pet_action_feedback(reason: u8, helpful: bool, errors: &mut UiErrorKeys) {
    debug!("net: pet action feedback {reason}");
    if let Some(key) = pet_feedback_key(reason, helpful) {
        errors.0.push(UiError::key(key));
    }
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
fn pet_name_invalid(code: u32, errors: &mut UiErrorKeys) {
    let key = NameError(code).key();
    debug!("net: pet name refused, code {code} ({key})");
    errors.0.push(UiError::key(key));
}

/// `SMSG_PET_BROKEN`, an empty body: loyalty hit zero; raise `ERR_PET_BROKEN`. The zero-guid
/// `SMSG_PET_SPELLS` of the unsummon right after it clears the bar (`Pet.cpp:822`).
fn pet_broken(errors: &mut UiErrorKeys) {
    debug!("net: pet ran away");
    errors.0.push(UiError::key("ERR_PET_BROKEN"));
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
        let mut errors = UiErrorKeys::default();
        pet_name_invalid(NameError::TOO_LONG.0, &mut errors);
        pet_name_invalid(99, &mut errors);
        let keys: Vec<&str> = errors.0.iter().map(|e| e.key).collect();
        assert_eq!(keys, ["ERR_NAME_TOO_LONG", "ERR_NAME_INVALID"]);
    }

    #[test]
    fn a_zero_guid_tears_the_whole_bar_down() {
        let mut bar = PetBar::default();
        pet_spells(a_bar(0x2A), None, &mut bar);
        assert_eq!(bar.spells.pet_guid, 0x2A);

        pet_spells(PetSpells::default(), None, &mut bar);
        assert_eq!(bar.spells, PetSpells::default());
        assert!(!bar.has_bar());
    }

    /// The pet setter: each packet re-arms the expiry from its own duration, 0 clears it, and the
    /// teardown clears it too.
    #[test]
    fn each_pet_packet_rearms_or_clears_the_expiry() {
        let mut bar = PetBar::default();
        let mut charm = a_bar(0x2A);
        charm.duration_ms = 60_000;
        let before = Instant::now();
        pet_spells(charm.clone(), None, &mut bar);
        let left = bar.expires.expect("armed").duration_since(before);
        assert!(
            left >= std::time::Duration::from_secs(60) && left < std::time::Duration::from_secs(61)
        );

        pet_spells(a_bar(0x2A), None, &mut bar);
        assert_eq!(bar.expires, None, "a 0 duration clears it");
        pet_spells(charm, None, &mut bar);
        assert!(bar.expires.is_some());
        pet_spells(PetSpells::default(), None, &mut bar);
        assert_eq!(bar.expires, None, "the teardown clears it");
    }

    #[test]
    fn pet_mode_only_writes_its_own_bar() {
        let mut bar = PetBar::default();
        pet_spells(a_bar(0x2A), None, &mut bar);

        pet_mode(
            PetMode {
                pet_guid: 0x99,
                state: 0x0202,
            },
            &mut bar,
        );
        assert_eq!(bar.spells.react_state(), 1, "a stranger's mode is ignored");

        pet_mode(
            PetMode {
                pet_guid: 0x2A,
                state: 0x0202,
            },
            &mut bar,
        );
        assert_eq!(bar.spells.react_state(), 2);
        assert_eq!(bar.spells.command_state(), 2);
        assert_eq!(bar.spells.bar[0].action(), PET_COMMAND_ATTACK);
    }

    /// The client's pet-guid writer clears the latch only on a change.
    #[test]
    fn the_attack_latch_survives_a_resend_and_dies_on_a_new_pet() {
        let mut bar = PetBar::default();
        pet_spells(a_bar(0x2A), None, &mut bar);
        bar.attacking = true;

        pet_spells(a_bar(0x2A), None, &mut bar);
        assert!(bar.attacking, "the same pet re-sending keeps its attack");

        pet_spells(a_bar(0x99), None, &mut bar);
        assert!(!bar.attacking, "a different pet is not attacking");
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
        let mut errors = UiErrorKeys::default();
        pet_spell_line(17253, true, Some(&spells), &mut errors);
        pet_spell_line(24450, true, Some(&spells), &mut errors);
        pet_spell_line(17253, false, Some(&spells), &mut errors);
        pet_spell_line(99999, true, Some(&spells), &mut errors);
        pet_spell_line(17253, true, None, &mut errors);
        let lines: Vec<(&str, Option<&str>)> =
            errors.0.iter().map(|e| (e.key, e.arg_s())).collect();
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
        pet_spells(a_bar(0x2A), None, &mut bar);
        assert_eq!(bar.pets, [0x2A]);

        pet_guids(vec![0x2A, 0x99, 0x77], &mut bar);
        assert_eq!(bar.spells.pet_guid, 0x2A);
        pet_spells(a_bar(0x77), None, &mut bar);
        assert_eq!(bar.pets, [0x77, 0x99, 0x2A], "a swap with the head");
        pet_spells(a_bar(0x55), None, &mut bar);
        assert_eq!(bar.pets, [0x55]);

        pet_guids(vec![0x99], &mut bar);
        assert_eq!(bar.spells.pet_guid, 0x99, "the head is the pet");
        pet_guids(Vec::new(), &mut bar);
        assert!(!bar.has_bar(), "an empty list is no pet");
        assert!(!bar.spells.bar[0].is_empty(), "the words stay");

        pet_spells(PetSpells::default(), None, &mut bar);
        assert!(bar.pets.is_empty());
    }

    #[test]
    fn the_feedback_map_is_the_switch_at_0x5d4dc8() {
        let mut errors = UiErrorKeys::default();
        for reason in [0u8, 1, 2, 3, 4, 5, 200] {
            pet_action_feedback(reason, false, &mut errors);
        }
        // A helpful spell's refusal reads as no valid targets; reason 3 ignores the spell.
        pet_action_feedback(2, true, &mut errors);
        pet_action_feedback(3, true, &mut errors);
        assert_eq!(
            errors.0.iter().map(|e| e.key).collect::<Vec<_>>(),
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
        pet_mode(mode(0xF14), &mut bar);
        let first = bar.bar_signals;
        pet_mode(mode(0xF14), &mut bar);
        assert_ne!(bar.bar_signals, first, "the same state signals again");
        let before = bar.bar_signals;
        pet_mode(mode(0xABC), &mut bar);
        assert_eq!(bar.bar_signals, before, "another pet's mode is not ours");
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
}
