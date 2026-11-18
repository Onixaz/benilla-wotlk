//! Characterization of the NPC service windows (#1279): their events fire from the packet handlers in wire order, and their verbs read the `World` at the call.

use benilla_protocol::field::{FIELD_UNIT_HEALTH, FIELD_UNIT_LEVEL, FIELD_UNIT_MAXHEALTH};
use benilla_protocol::messages::ObjectType;
use benilla_protocol::{EntityKind, ObjectFields, SessionEvent};

use super::characterization_tests::{pin, self_values, Rig, OBJECT_TYPE_PLAYER};
use crate::net::ClientCommand;

/// The NPC's guid.
const NPC: u64 = 0xF130_0000_0300_0001;
/// A second NPC.
const NPC2: u64 = 0xF130_0000_0300_0002;

/// A creature with `npc_flags` standing at `position`; ours is `[-8949.95, -132.493, 83.5312]`.
fn npc_at(guid: u64, npc_flags: u32, position: [f32; 3]) -> SessionEvent {
    npc_with(guid, npc_flags, position, &[])
}

/// [`npc_at`] with `extra` fields.
fn npc_with(guid: u64, npc_flags: u32, position: [f32; 3], extra: &[(u16, u32)]) -> SessionEvent {
    let mut pairs = vec![
        (2, 0x9),
        (FIELD_UNIT_HEALTH, 500),
        (FIELD_UNIT_MAXHEALTH, 500),
        (FIELD_UNIT_LEVEL, 30),
        (benilla_protocol::field::FIELD_UNIT_NPC_FLAGS, npc_flags),
    ];
    pairs.extend_from_slice(extra);
    SessionEvent::ObjectCreate {
        guid,
        kind: EntityKind::Unit,
        display_id: None,
        position,
        orientation: 0.0,
        scale: 1.0,
        speeds: None,
        mover: None,
        transport_progress: None,
        transport: None,
        spline: None,
        vehicle: None,
        attack_target: None,
        spawned: true,
        fields: ObjectFields::from_pairs(&pairs).into_created(ObjectType::Unit),
    }
}

/// Beside us, inside the 4 yd a zero combat reach leaves.
const NEAR: [f32; 3] = [-8949.0, -132.0, 83.5];
/// 20 yd off.
const FAR: [f32; 3] = [-8929.0, -132.0, 83.5];

/// The commands sent from here on.
fn tap(rig: &mut Rig) -> crossbeam_channel::Receiver<ClientCommand> {
    let (tx, rx) = crossbeam_channel::unbounded();
    rig.app.insert_resource(crate::net::NetCommands(tx));
    rx
}

/// Run `chunk` with the `World` lent, as a click's script call; the error lines it showed, then `returned`.
fn call(rig: &mut Rig, chunk: &str) -> String {
    super::live::with_live_world(rig.app.world_mut(), |s| {
        s.eval::<String>(&format!(
            "local before = #(CHARZ_LOG or {{}}) {chunk} local t = {{}} \
             for i = before + 1, #(CHARZ_LOG or {{}}) do \
               if CHARZ_LOG[i]:find('UI_ERROR_MESSAGE', 1, true) then \
                 t[#t + 1] = CHARZ_LOG[i]:match('\t([^\t]*)\t') end end \
             t[#t + 1] = 'returned' return table.concat(t, '|')"
        ))
    })
    .expect("a VM")
    .expect("the chunk runs")
}

/// `SMSG_BINDER_CONFIRM` runs the binder talk in its handler (`0x6e3db9` -> `0x6d2120`): an innkeeper not held, or out of its reach, latches nothing and asks nothing; one in reach latches and fires `CONFIRM_BINDER`, `CheckBinderDist` reading the latch at the event (`0x6d2290`). The create after the confirm in one frame finds nothing held at the confirm.
#[test]
fn the_binder_confirm_asks_only_for_a_held_innkeeper_in_reach() {
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    let events = &["CONFIRM_BINDER"];
    let verbs = &["binderdist"];
    let seen = rig.packets(
        vec![
            SessionEvent::BinderConfirm { binder: NPC },
            npc_at(NPC, 0x10000, NEAR),
        ],
        2,
    );
    assert!(
        pin(&seen, events, verbs).is_empty(),
        "not held at the confirm"
    );
    assert_eq!(rig.live_now().verbs["binderdist"], "nil");
    let seen = rig.packets(vec![SessionEvent::BinderConfirm { binder: NPC }], 2);
    let pinned = pin(&seen, events, verbs);
    assert_eq!(pinned.len(), 1, "{pinned:?}");
    assert!(pinned[0].ends_with("| binderdist=1"), "{pinned:?}");
    // An innkeeper out of reach asks nothing and keeps the old latch.
    let seen = rig.packets(
        vec![
            npc_at(NPC2, 0x10000, FAR),
            SessionEvent::BinderConfirm { binder: NPC2 },
        ],
        2,
    );
    assert!(pin(&seen, events, verbs).is_empty(), "out of reach");
    assert_eq!(rig.live_now().verbs["binderdist"], "1");
    // The innkeeper leaves: the latch stays, the distance reads it live.
    rig.packets(vec![SessionEvent::ObjectDestroyed(NPC)], 2);
    assert_eq!(rig.live_now().verbs["binderdist"], "nil");
}

/// `ConfirmBinder` is the talk with no guid (`0x51bfc3`): the latched innkeeper in reach sends `CMSG_BINDER_ACTIVATE` inside the call (`0x6d2257`), out of reach nothing; the latch is never cleared, so a second accept sends again.
#[test]
fn confirm_binder_sends_inside_its_call_while_in_reach() {
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    rig.packets(
        vec![
            npc_at(NPC, 0x10000, NEAR),
            SessionEvent::BinderConfirm { binder: NPC },
        ],
        2,
    );
    let rx = tap(&mut rig);
    let sends = |rx: &crossbeam_channel::Receiver<ClientCommand>| {
        rx.try_iter()
            .filter(|c| matches!(c, ClientCommand::BinderActivate { binder } if *binder == NPC))
            .count()
    };
    call(&mut rig, "ConfirmBinder()");
    assert_eq!(sends(&rx), 1);
    call(&mut rig, "ConfirmBinder()");
    assert_eq!(sends(&rx), 1, "the latch outlives the accept");
    rig.packets(vec![SessionEvent::ObjectDestroyed(NPC)], 2);
    call(&mut rig, "ConfirmBinder()");
    assert_eq!(sends(&rx), 0, "not held");
}

/// `MSG_TALENT_WIPE_CONFIRM`'s handler (`0x6e3d4a`): a zero guid shows `ERR_TALENT_WIPE_ERROR` in its turn (`0x6e3d82`); a trainer in reach latches and fires `CONFIRM_TALENT_WIPE` with the cost (`0x6db28d`), `CheckTalentMasterDist` reading the latch at the event (`0x6d2070`).
#[test]
fn the_talent_wipe_confirm_asks_from_its_handler() {
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    let events = &["CONFIRM_TALENT_WIPE", "UI_ERROR_MESSAGE"];
    let verbs = &["talentdist"];
    let seen = rig.packets(
        vec![
            SessionEvent::TalentWipeConfirm {
                trainer: 0,
                cost: 0,
            },
            npc_at(NPC, 0x10, NEAR),
            SessionEvent::TalentWipeConfirm {
                trainer: NPC,
                cost: 10_000,
            },
        ],
        2,
    );
    assert_eq!(
        pin(&seen, events, verbs),
        [
            "UI_ERROR_MESSAGE You have not spent any talent points. | talentdist=nil",
            "CONFIRM_TALENT_WIPE 10000 | talentdist=1",
        ]
    );
}

/// `ConfirmTalentWipe` is the talk with no guid (`0x51bf84`): our coinage below the latched cost shows `ERR_NOT_ENOUGH_MONEY` inside the call (`0x6db2ae`); at the cost the wipe goes out naming the latch.
#[test]
fn confirm_talent_wipe_refuses_or_sends_inside_its_call() {
    use benilla_protocol::field::FIELD_PLAYER_FIELD_COINAGE;
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    rig.packets(
        vec![
            npc_at(NPC, 0x10, NEAR),
            SessionEvent::TalentWipeConfirm {
                trainer: NPC,
                cost: 10_000,
            },
            self_values(&[(FIELD_PLAYER_FIELD_COINAGE, 9_999)]),
        ],
        2,
    );
    let rx = tap(&mut rig);
    let sends = |rx: &crossbeam_channel::Receiver<ClientCommand>| {
        rx.try_iter()
            .filter(
                |c| matches!(c, ClientCommand::TalentWipeConfirm { trainer } if *trainer == NPC),
            )
            .count()
    };
    assert_eq!(
        call(&mut rig, "ConfirmTalentWipe()"),
        "You don't have enough money.|returned"
    );
    assert_eq!(sends(&rx), 0);
    rig.packets(
        vec![self_values(&[(FIELD_PLAYER_FIELD_COINAGE, 10_000)])],
        1,
    );
    assert_eq!(call(&mut rig, "ConfirmTalentWipe()"), "returned");
    assert_eq!(sends(&rx), 1);
}

/// `SMSG_RAID_GROUP_ONLY`: a zero delay shows its reason's line first (`0x6e3be3`), then the boot setter clears or arms the deadline and fires its event in the handler's turn (`0x6e3c10` -> `0x513ad0`), `GetInstanceBootTimeRemaining` reading the deadline just set (`0x5162e0`).
#[test]
fn the_instance_boot_fires_from_its_handler_with_the_deadline_set() {
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    let events = &[
        "INSTANCE_BOOT_START",
        "INSTANCE_BOOT_STOP",
        "UI_ERROR_MESSAGE",
    ];
    let verbs = &["boot"];
    let seen = rig.packets(
        vec![
            SessionEvent::RaidGroupOnly {
                delay_ms: 60_500,
                reason: 0,
            },
            SessionEvent::RaidGroupOnly {
                delay_ms: 0,
                reason: 2,
            },
            SessionEvent::RaidGroupOnly {
                delay_ms: 30_500,
                reason: 0,
            },
        ],
        2,
    );
    assert_eq!(
        pin(&seen, events, verbs),
        [
            "INSTANCE_BOOT_START | boot=60",
            "UI_ERROR_MESSAGE You must be in a raid group to enter this instance. | boot=60",
            "INSTANCE_BOOT_STOP | boot=0",
            "INSTANCE_BOOT_START | boot=30",
        ]
    );
}

/// `SMSG_AREA_SPIRIT_HEALER_TIME` runs the healer poll first (`0x524b10` -> `0x524010`), so a guide streamed in the same frame is adopted before the time is matched; the held healer's positive time arms the wave and fires `AREA_SPIRIT_HEALER_IN_RANGE` with `GetAreaSpiritHealerTime` reading it (`0x516b90`).
#[test]
fn the_spirit_healer_time_polls_first_and_fires_in_range() {
    use benilla_protocol::field::{
        FIELD_PLAYER_FLAGS, FIELD_UNIT_FACTIONTEMPLATE, FIELD_UNIT_FLAGS,
    };
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER, (FIELD_UNIT_FACTIONTEMPLATE, 1)]) else {
        return;
    };
    let guide = || {
        npc_with(
            NPC,
            0x8000,
            NEAR,
            &[(FIELD_UNIT_FACTIONTEMPLATE, 35), (FIELD_UNIT_FLAGS, 0x1000)],
        )
    };
    let events = &[
        "AREA_SPIRIT_HEALER_IN_RANGE",
        "AREA_SPIRIT_HEALER_OUT_OF_RANGE",
    ];
    let verbs = &["spirittime"];
    let time = SessionEvent::AreaSpiritHealerTime {
        healer: NPC,
        ms: 30_500,
    };
    // Alive: the poll holds no healer, so the time arms nothing.
    let seen = rig.packets(vec![guide(), time.clone()], 2);
    assert!(pin(&seen, events, verbs).is_empty(), "{seen:?}");
    // A ghost: the guide, our ghost bit and the time in one frame.
    let seen = rig.packets(
        vec![self_values(&[(FIELD_PLAYER_FLAGS, 0x10)]), time.clone()],
        2,
    );
    assert_eq!(
        pin(&seen, events, verbs),
        ["AREA_SPIRIT_HEALER_IN_RANGE | spirittime=30"]
    );
    // Alive again: the frame's poll drops the healer, cancelling the wave.
    let seen = rig.packets(vec![self_values(&[(FIELD_PLAYER_FLAGS, 0)])], 2);
    assert_eq!(
        pin(&seen, events, verbs),
        ["AREA_SPIRIT_HEALER_OUT_OF_RANGE | spirittime=0"]
    );
}
