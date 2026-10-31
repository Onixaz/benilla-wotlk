use super::*;

use benilla_protocol::messages::{EncounterUnitUpdate as U, InstanceResetFailed};

/// The shipped 3.3.5a templates from `GlobalStrings.lua`, so these tests need no install.
fn template(token: &str) -> String {
    match token {
        "RAID_INSTANCE_WARNING_HOURS" => {
            "Your instance lock for %s will expire in %d |4hour:hours;."
        }
        "RAID_INSTANCE_WARNING_MIN" => {
            "Your instance lock for %s will expire in %d |4minute:minutes;!"
        }
        "RAID_INSTANCE_EXPIRED" => "Your instance lock for %s has expired.",
        "INSTANCE_RESET_SUCCESS" => "%s has been reset.",
        "INSTANCE_RESET_FAILED" => "Cannot reset %s.  There are players still inside the instance.",
        "INSTANCE_SAVED" => "You are now saved to this instance",
        "RESET_FAILED_NOTIFY" => {
            "The party leader has attempted to reset the instance you are in.  Please zone out to \
             allow the instance to reset."
        }
        "DUNGEON_NAME_WITH_DIFFICULTY" => "%1$s (%2$s)",
        "RAID_DIFFICULTY_25PLAYER" => "25 Player",
        _ => "",
    }
    .to_string()
}

fn text_of(line: &LockoutLine, name: Option<&str>) -> Option<String> {
    lockout_text(line, name, &template(line.token))
}

fn message(message_type: u32, seconds: i32, welcome: Option<(bool, bool)>) -> RaidInstanceMessage {
    RaidInstanceMessage {
        message_type,
        map: 631,
        difficulty: 1,
        seconds,
        welcome,
    }
}

#[test]
fn countdown_lines_take_the_truncated_count_and_name_the_difficulty() {
    let hours = raid_instance_line(&message(1, 2 * 3_600 + 59, None)).expect("type 1");
    assert_eq!(hours.numbers, vec![2]);
    assert_eq!(hours.map, LineMap::WithDifficulty(631, 1));
    assert_eq!(
        text_of(&hours, Some("Icecrown Citadel (25 Player)")).as_deref(),
        Some("Your instance lock for Icecrown Citadel (25 Player) will expire in 2 |4hour:hours;.")
    );
    let mins = raid_instance_line(&message(2, 5 * 60 + 30, None)).expect("type 2");
    assert_eq!(mins.numbers, vec![5]);
    // Signed: a negative count truncates toward zero, as `imul`/`sar` does.
    let negative = raid_instance_line(&message(3, -90, None)).expect("type 3");
    assert_eq!(negative.numbers, vec![-1]);
    let expired = raid_instance_line(&message(5, 0, None)).expect("type 5");
    assert!(expired.numbers.is_empty());
    assert_eq!(
        text_of(&expired, Some("ICC")).as_deref(),
        Some("Your instance lock for ICC has expired.")
    );
}

#[test]
fn the_welcome_is_an_event_and_other_types_are_silent() {
    let mut state = InstanceState::default();
    net::raid_instance_message(&mut state, message(4, 3_600, Some((true, false))));
    assert!(state.lines.is_empty(), "no line: UIParent.lua words it");
    assert_eq!(state.welcomes, vec![(631, 1, 3_600, true, false)]);
    for ty in [0, 6, u32::MAX] {
        net::raid_instance_message(&mut state, message(ty, 60, None));
    }
    assert!(state.lines.is_empty());
    assert_eq!(state.welcomes.len(), 1);
}

#[test]
fn the_map_name_takes_its_difficulty_row() {
    let text = |t: &str| template(t);
    assert_eq!(
        map_name_with_difficulty(631, 1, None, None, &text),
        "",
        "no map row is an empty name, not the id"
    );
    assert_eq!(map_name(9999, None), "9999", "the reset lines print the id");
}

#[test]
fn reset_lines_and_the_failure_notice() {
    let mut state = InstanceState::default();
    net::instance_reset(&mut state, 36);
    net::instance_reset_failed(&mut state, InstanceResetFailed { reason: 0, map: 36 });
    // Reason 3 and up: no line, where the reference prints a stack buffer.
    net::instance_reset_failed(&mut state, InstanceResetFailed { reason: 3, map: 36 });
    net::reset_failed_notify(&mut state);
    let texts: Vec<_> = std::mem::take(&mut state.lines)
        .iter()
        .map(|l| {
            let name = matches!(l.map, LineMap::Name(_)).then_some("Deadmines");
            text_of(l, name).unwrap()
        })
        .collect();
    assert_eq!(
        texts,
        [
            "Deadmines has been reset.",
            "Cannot reset Deadmines.  There are players still inside the instance.",
            "The party leader has attempted to reset the instance you are in.  Please zone out \
             to allow the instance to reset.",
        ]
    );
}

#[test]
fn save_created_stops_the_lock_clock_on_flag_zero_only() {
    let now = Instant::now();
    let mut state = InstanceState::default();
    net::instance_lock_warning(
        &mut state,
        InstanceLockWarning {
            ms: 60_000,
            completed_mask: 1,
            extending: false,
        },
        now,
    );
    net::instance_save_created(&mut state, 1);
    assert!(
        state.lock.read(now).running,
        "the debug arm leaves the clock"
    );
    net::instance_save_created(&mut state, 0);
    assert!(!state.lock.read(now).running);
    assert_eq!(
        state.lock_events,
        ["INSTANCE_LOCK_START", "INSTANCE_LOCK_STOP"]
    );
    let lines = std::mem::take(&mut state.lines);
    assert_eq!(
        text_of(&lines[0], None).as_deref(),
        Some("(Debug-Only Lock Notice) You are now saved to this instance")
    );
    assert_eq!(
        text_of(&lines[1], None).as_deref(),
        Some("You are now saved to this instance")
    );
    net::instance_save_created(&mut state, 2);
    assert!(state.lines.is_empty(), "flag 2 has no arm");
}

/// `0x513b20` and `0x513b90`: a positive time arms, the reads truncate to seconds, the deadline
/// clears every field, and zero or a negative `i32` stops.
#[test]
fn the_lock_clock_arms_reads_and_stops() {
    let now = Instant::now();
    let mut clock = LockClock::default();
    let warning = |ms| InstanceLockWarning {
        ms,
        completed_mask: 0b101,
        extending: true,
    };
    assert_eq!(clock.set(warning(60_500), now), "INSTANCE_LOCK_START");
    assert_eq!(
        clock.read(now),
        InstanceLock {
            running: true,
            secs: 60,
            completed_mask: 0b101,
            extending: true,
        }
    );
    assert_eq!(
        clock.read(now + Duration::from_millis(60_100)),
        InstanceLock {
            running: true,
            secs: 0,
            completed_mask: 0b101,
            extending: true,
        },
        "under a second left still runs"
    );
    assert_eq!(
        clock.read(now + Duration::from_secs(61)),
        InstanceLock::default()
    );
    assert_eq!(clock.set(warning(0x8000_0000), now), "INSTANCE_LOCK_STOP");
    assert_eq!(clock.read(now), InstanceLock::default());
    clock.set(warning(1_000), now);
    assert_eq!(clock.set(warning(0), now), "INSTANCE_LOCK_STOP");
}

#[test]
fn leaving_the_world_stops_the_lock_clock() {
    let now = Instant::now();
    let mut state = InstanceState::default();
    net::instance_lock_warning(
        &mut state,
        InstanceLockWarning {
            ms: 60_000,
            completed_mask: 0,
            extending: false,
        },
        now,
    );
    state.leave_world();
    assert!(!state.lock.read(now).running);
    assert_eq!(
        state.lock_events,
        ["INSTANCE_LOCK_START", "INSTANCE_LOCK_STOP"]
    );
}

fn engaged(units: &EncounterUnits) -> Vec<u64> {
    units.guids().take_while(|&g| g != 0).collect()
}

/// `0x5ed7b0`, `0x5ed870` and the sort `0x5ed750`: held slots first by priority then arrival, a
/// repeat engage ignored, one signal per change, and the slots gone once the first is empty.
#[test]
fn encounter_slots_sort_by_priority_then_arrival() {
    let mut units = EncounterUnits::default();
    assert_eq!(units.boss(0), 0, "no slots before the first engage");
    units.apply(U::Engage {
        guid: 0xa,
        priority: 2,
    });
    units.apply(U::Engage {
        guid: 0xb,
        priority: 1,
    });
    units.apply(U::Engage {
        guid: 0xc,
        priority: 2,
    });
    units.apply(U::Engage {
        guid: 0xb,
        priority: 0,
    });
    assert_eq!(units.slots.len(), BOSS_SLOTS);
    assert_eq!(engaged(&units), [0xb, 0xa, 0xc]);
    assert_eq!(units.signals, 3, "the repeat engage is silent");

    units.apply(U::Priority {
        guid: 0xc,
        priority: 0,
    });
    assert_eq!(engaged(&units), [0xc, 0xb, 0xa]);
    units.apply(U::Priority {
        guid: 0xdead,
        priority: 0,
    });
    assert_eq!(units.signals, 4, "an unknown unit is not re-ranked");

    units.apply(U::Disengage { guid: 0xb });
    assert_eq!(engaged(&units), [0xc, 0xa]);
    assert_eq!(units.boss(2), 0);
    units.apply(U::Resort);
    units.apply(U::Timer { ms: 10 });
    units.apply(U::EnableObjective { id: 1 });
    assert_eq!(units.signals, 6, "the objectives signal nothing");

    units.apply(U::Disengage { guid: 0xc });
    units.apply(U::Disengage { guid: 0xa });
    assert!(units.slots.is_empty(), "the first slot empty: the slots go");
    assert_eq!(units.signals, 8);
    units.apply(U::Disengage { guid: 0xa });
    assert_eq!(units.signals, 8, "a unit not held is silent");
}

#[test]
fn the_encounter_world_leave_drops_the_slots_silently() {
    let mut units = EncounterUnits::default();
    units.apply(U::Engage {
        guid: 0xa,
        priority: 1,
    });
    units.clear();
    assert!(units.slots.is_empty());
    assert_eq!(units.signals, 0);
}

/// The test world: 36 a 5-man that resets, 631 a raid with a lock, 0 a continent.
fn facts() -> MapFacts<'static> {
    MapFacts {
        known: &|m| matches!(m, 0 | 36 | 631),
        resettable: &|m| m == 36,
    }
}

#[test]
fn the_latch_records_only_a_resettable_instance_we_are_not_in() {
    let mut state = InstanceState::default();
    state.note_last_instance(631, Some(0), false, 100, LatchWriter::Packet);
    assert_eq!(state.last_instance, None, "a locked raid is not resettable");
    state.note_last_instance(36, Some(36), true, 100, LatchWriter::Packet);
    assert_eq!(state.last_instance, None, "the instance we stand in");
    state.note_last_instance(36, Some(0), true, 100, LatchWriter::Packet);
    assert_eq!(
        (state.last_instance, state.last_instance_at),
        (Some(36), 100)
    );
    assert!(!state.saw_own_dungeon, "the packet is the server's word");
    state.note_last_instance(36, Some(0), true, 100, LatchWriter::WorldEntry);
    assert!(state.saw_own_dungeon);
}

/// `CanShowResetInstances()`'s terms one at a time, with the witness standing in for the bind the
/// server never reports.
#[test]
fn can_reset_needs_every_term() {
    let f = facts();
    let mut state = InstanceState::default();
    state.note_last_instance(36, Some(0), true, 100, LatchWriter::Packet);
    assert!(!state.can_reset(Some(0), &f, 100), "1 · no bind");
    state.set_ownership(true);
    assert!(state.can_reset(Some(0), &f, 100));
    assert!(state.can_reset(Some(0), &f, 100 + RESET_OFFER_WINDOW_SECS));
    assert!(
        !state.can_reset(Some(0), &f, 101 + RESET_OFFER_WINDOW_SECS),
        "5 · past the window"
    );
    assert!(!state.can_reset(Some(36), &f, 100), "2 · standing inside");
    assert!(
        state.can_reset(Some(631), &f, 100),
        "a locked raid does not suppress the row"
    );
    state.clear_last_instance();
    assert!(!state.can_reset(Some(0), &f, 100), "3 · reset landed");

    // The witness alone meets term 1, and goes with a reset or a logout.
    let mut state = InstanceState::default();
    state.note_last_instance(36, Some(0), true, 100, LatchWriter::WorldEntry);
    assert!(state.can_reset(Some(0), &f, 100));
    state.forget_witness();
    assert!(!state.can_reset(Some(0), &f, 100));
}

/// The resettable test at the current difficulty (`0x5195f1`-`0x519616`), off the real tables:
/// a 5-man has no lock at normal, a raid has one, and a difficulty past 3 always resets. Skips
/// without client data.
#[test]
fn resettable_reads_map_difficulty() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let maps = benilla_assets::MapCatalogRes(
        benilla_formats::load_map_catalog(&mut chain).expect("Map.dbc"),
    );
    let rows = crate::ui_transfer::MapDifficulties(
        benilla_formats::load_map_difficulties(&mut chain).expect("MapDifficulty.dbc"),
    );
    assert!(resettable(36, Some(&maps), Some(&rows), 0), "Deadmines");
    assert!(
        !resettable(631, Some(&maps), Some(&rows), 0),
        "Icecrown Citadel locks"
    );
    assert!(resettable(631, Some(&maps), Some(&rows), 4));
    assert!(!resettable(0, Some(&maps), Some(&rows), 0), "a continent");
    assert!(maps.0.dynamic_difficulty_maps().contains(&631));
}

/// Every token the module prints, and the welcome's, resolves in the player's 3.3.5a
/// `GlobalStrings.lua` with the fills it is given. Skips without client data.
#[test]
fn every_instance_token_resolves_in_the_real_global_strings() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let src = chain
        .read_file("Interface\\FrameXML\\GlobalStrings.lua")
        .expect("GlobalStrings.lua in the chain");
    let s = UiScript::new().expect("VM");
    s.run(&String::from_utf8_lossy(&src)).expect("runs clean");
    for token in [
        "RAID_INSTANCE_WARNING_HOURS",
        "RAID_INSTANCE_WARNING_MIN",
        "RAID_INSTANCE_WARNING_MIN_SOON",
        "RAID_INSTANCE_EXPIRED",
        "INSTANCE_RESET_SUCCESS",
        "INSTANCE_RESET_FAILED",
        "INSTANCE_RESET_FAILED_OFFLINE",
        "INSTANCE_RESET_FAILED_ZONING",
        "INSTANCE_SAVED",
        "RESET_FAILED_NOTIFY",
        "DUNGEON_NAME_WITH_DIFFICULTY",
    ] {
        assert!(!s.engine_text(token).is_empty(), "{token}");
    }
    for ty in [1u32, 2, 3, 5] {
        let line = raid_instance_line(&message(ty, 3_600, None)).expect("a line");
        let template = s.engine_text(line.token);
        assert_eq!(
            template.matches("%d").count(),
            line.numbers.len(),
            "{} takes {} number(s)",
            line.token,
            line.numbers.len()
        );
        assert!(template.contains("%s"), "{} names the instance", line.token);
    }
}
