//! The group packet handlers. [`GroupState`] names the messages each packet implies and these queue
//! them by key, as the reference's error display takes a message id: the catalog row gives the
//! text, the surface and the sound. [`member_deactivated`] and [`roster_deactivated`] are the
//! object layer's hooks on a roster member's stream-out.

use benilla_protocol::messages::{GroupLootInfo, GroupMemberEntry, PartyMemberStatsInfo};
use bevy::prelude::*;

use benilla_protocol::{SessionEvent, SessionEventKind};

use super::difficulty::RaidMap;
use super::{GroupNotices, GroupState, TargetIconNotice, GROUPTYPE_RAID};
use crate::names::NameCache;
use crate::net::{ClientCommand, GuidIndex, NetCommands, NetHandlerApp, ObjectStore, SelfGuid};
use crate::sound::MessageSounds;
use crate::ui_action::{UiError, UiErrorKeys};
use crate::ui_quest::QuestGiver;
use crate::ui_summon::SummonState;
use crate::unit_auras::UnitAuras;
use benilla_protocol::messages::{party_operation, party_result};

/// The join chime, played by name.
const INVITE_ACCEPT_SOUND: &str = "igPlayerInviteAccept";

/// One handler per group kind, plus the session-end listener.
pub(super) fn register(app: &mut App) {
    use SessionEventKind as K;
    app.net_handler(K::GroupInvite, on_invite)
        .net_handler(K::GroupDecline, on_decline)
        .net_handler(K::GroupCancel, on_cancel)
        .net_handler(K::GroupActionThrottled, on_keyed_notice)
        .net_handler(K::ReadyCheckError, on_keyed_notice)
        .net_handler(K::PartySquelch, on_keyed_notice)
        .net_handler(K::EchoPartySquelch, on_echo_squelch)
        .net_handler(K::GroupUninvited, on_uninvited)
        .net_handler(K::GroupLeaderChanged, on_leader_changed)
        .net_handler(K::GroupDestroyed, on_destroyed)
        .net_handler(K::GroupList, on_list)
        .net_handler(K::RealGroupUpdate, on_real_group_update)
        .net_handler(K::PartyCommandResult, on_command_result)
        .net_handler(K::PartyMemberStats, on_member_stats)
        .net_handler(K::RaidTargetSet, on_raid_target)
        .net_handler(K::RaidTargetList, on_raid_target)
        .net_handler(K::ReadyCheckStarted, on_ready_check)
        .net_handler(K::ReadyCheckConfirm, on_ready_check)
        .net_handler(K::ReadyCheckFinished, on_ready_check)
        .net_handler(K::RaidInstanceInfo, on_raid_instance_info)
        .net_handler(K::DungeonDifficulty, on_difficulty)
        .net_handler(K::RaidDifficulty, on_difficulty)
        .net_handler(K::Disconnected, on_session_end);
}

/// The current map's raid wording, off `Map.dbc`.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct CurrentRaidMap<'w> {
    maps: Option<Res<'w, benilla_assets::MapCatalogRes>>,
    here: Option<Res<'w, benilla_world::world_map::CurrentMap>>,
}

impl CurrentRaidMap<'_> {
    pub(super) fn kind(&self) -> RaidMap {
        RaidMap::of(self.here.as_ref().map(|m| m.0), self.maps.as_deref())
    }
}

/// `MSG_SET_DUNGEON_DIFFICULTY` and `MSG_SET_RAID_DIFFICULTY` (both arms of `0x526530`).
fn on_difficulty(
    In(ev): In<SessionEvent>,
    mut group: ResMut<GroupState>,
    mut errors: ResMut<UiErrorKeys>,
    map: CurrentRaidMap,
) {
    let lines = match ev {
        SessionEvent::DungeonDifficulty(set) => group.apply_dungeon_difficulty(set),
        SessionEvent::RaidDifficulty(set) => group.apply_raid_difficulty(set, map.kind()),
        _ => return,
    };
    push_group_lines(&mut errors, lines);
}

fn on_invite(
    In(ev): In<SessionEvent>,
    mut group: ResMut<GroupState>,
    mut errors: ResMut<UiErrorKeys>,
) {
    if let SessionEvent::GroupInvite {
        can_accept,
        inviter,
    } = ev
    {
        invited(&mut group, &mut errors, can_accept, &inviter);
    }
}

fn on_decline(
    In(ev): In<SessionEvent>,
    mut group: ResMut<GroupState>,
    mut errors: ResMut<UiErrorKeys>,
) {
    if let SessionEvent::GroupDecline { name } = ev {
        declined(&mut group, &mut errors, &name);
    }
}

/// `SMSG_GROUP_CANCEL` (`0x6cbd10`): the invite is gone, and the invite edge fires
/// `PARTY_INVITE_CANCEL`, which hides the popup.
fn on_cancel(In(ev): In<SessionEvent>, mut group: ResMut<GroupState>) {
    if let SessionEvent::GroupCancel { .. } = ev {
        group.pending_invite = None;
    }
}

/// The notices that read at most a flag and show one line: the throttles `0x6cbe80` and
/// `0x5725a0`, and our own silencing, `0x5725c0` (nonzero silenced).
fn on_keyed_notice(In(ev): In<SessionEvent>, mut errors: ResMut<UiErrorKeys>) {
    let key = match ev {
        SessionEvent::GroupActionThrottled => "ERR_GROUP_ACTION_THROTTLED",
        SessionEvent::ReadyCheckError => "ERR_READY_CHECK_THROTTLED",
        SessionEvent::PartySquelch { silenced: true } => "ERR_PLAYER_SILENCED",
        SessionEvent::PartySquelch { silenced: false } => "ERR_PLAYER_UNSILENCED",
        _ => return,
    };
    errors.0.push(UiError::key(key));
}

/// `SMSG_ECHO_PARTY_SQUELCH` (`0x572610`): the member's cached name, never queried (`0x57264c`),
/// fills the echo; with none cached nothing shows.
fn on_echo_squelch(
    In(ev): In<SessionEvent>,
    mut errors: ResMut<UiErrorKeys>,
    names: Res<NameCache>,
) {
    if let SessionEvent::EchoPartySquelch { guid, silenced } = ev {
        let Some(name) = names.peek(guid) else {
            return;
        };
        let key = if silenced {
            "ERR_PLAYER_SILENCED_ECHO"
        } else {
            "ERR_PLAYER_UNSILENCED_ECHO"
        };
        errors.0.push(UiError::s(key, name));
    }
}

/// `SMSG_GROUP_UNINVITE` (`0x6cbe10`): the party clear-all, its line, then `CANCEL_SUMMON`.
fn on_uninvited(
    In(ev): In<SessionEvent>,
    mut group: ResMut<GroupState>,
    mut errors: ResMut<UiErrorKeys>,
    mut summon: ResMut<SummonState>,
    map: CurrentRaidMap,
) {
    if let SessionEvent::GroupUninvited = ev {
        let before = removal_start(&group);
        let lines = group.apply_uninvited();
        removal_end(&mut group, &mut errors, before, map.kind());
        push_group_lines(&mut errors, lines);
        summon.cancel();
    }
}

/// The current difficulties before the party clear-all empties the party (`0x52d6e8`).
fn removal_start(group: &GroupState) -> (u32, u32) {
    (
        group.current_dungeon_difficulty(),
        group.current_raid_difficulty(),
    )
}

/// `RemoveAll(1)`'s tail: the LFG bit cleared without a signal (`0x52d85e`), then the lines for a
/// difficulty the emptied party changed (`0x52d87b`).
fn removal_end(
    group: &mut GroupState,
    errors: &mut UiErrorKeys,
    (dungeon, raid): (u32, u32),
    map: RaidMap,
) {
    group.lfg_restricted = false;
    let lines = group.difficulty_lines(dungeon, raid, map);
    push_group_lines(errors, lines);
}

fn on_leader_changed(
    In(ev): In<SessionEvent>,
    mut group: ResMut<GroupState>,
    mut errors: ResMut<UiErrorKeys>,
    self_guid: Res<SelfGuid>,
    names: Res<NameCache>,
    commands: Res<NetCommands>,
) {
    if let SessionEvent::GroupLeaderChanged { name } = ev {
        leader_changed(
            &mut group,
            &mut errors,
            &name,
            &self_guid,
            &names,
            &commands,
        );
    }
}

/// `SMSG_GROUP_DESTROYED` (`0x6cbe40`): its line, the party clear-all, then `CANCEL_SUMMON`.
fn on_destroyed(
    In(ev): In<SessionEvent>,
    mut group: ResMut<GroupState>,
    mut errors: ResMut<UiErrorKeys>,
    mut summon: ResMut<SummonState>,
    map: CurrentRaidMap,
) {
    if let SessionEvent::GroupDestroyed = ev {
        let before = removal_start(&group);
        destroyed(&mut group, &mut errors);
        removal_end(&mut group, &mut errors, before, map.kind());
        summon.cancel();
    }
}

fn on_list(
    In(ev): In<SessionEvent>,
    mut group: ResMut<GroupState>,
    mut errors: ResMut<UiErrorKeys>,
    mut sounds: ResMut<MessageSounds>,
    mut quest: ResMut<QuestGiver>,
    self_guid: Res<SelfGuid>,
    names: Res<NameCache>,
    index: Res<GuidIndex>,
    commands: Res<NetCommands>,
    // Real: the stale window is wall-clock seconds (`0x6d8945`).
    clock: Res<Time<Real>>,
) {
    if let SessionEvent::GroupList {
        group_type,
        own_flags,
        own_roles,
        members,
        leader,
        loot,
        difficulties,
        lfg_restricted,
        battleground,
        group_guid,
        counter,
        ..
    } = ev
    {
        if group.list_is_stale(battleground, group_guid, counter, clock.elapsed_secs_f64()) {
            debug!("ui_party: stale group list {group_guid:#x} #{counter} skipped");
            return;
        }
        group.own_roles = own_roles;
        group.apply_list_tail(difficulties, lfg_restricted);
        let rows = members.len();
        list(
            &mut group,
            &mut errors,
            &mut sounds,
            &mut quest,
            group_type,
            own_flags,
            members,
            leader,
            loot,
            &self_guid,
            &names,
            &index,
            &commands,
        );
        if !battleground {
            group.set_real_counts(group_type == GROUPTYPE_RAID, rows);
        }
    }
}

fn on_real_group_update(In(ev): In<SessionEvent>, mut group: ResMut<GroupState>) {
    if let SessionEvent::RealGroupUpdate { flags, count, .. } = ev {
        group.apply_real_group_update(flags, count);
    }
}

/// `SMSG_PARTY_COMMAND_RESULT` (`0x6cbec0`); our leave's ack runs the party clear-all before its
/// line and ends in `CANCEL_SUMMON` (`0x6cbf14`-`0x6cbf2e`).
fn on_command_result(
    In(ev): In<SessionEvent>,
    mut group: ResMut<GroupState>,
    mut errors: ResMut<UiErrorKeys>,
    mut summon: ResMut<SummonState>,
    map: CurrentRaidMap,
) {
    if let SessionEvent::PartyCommandResult {
        operation,
        member,
        result,
        value,
    } = ev
    {
        let before = removal_start(&group);
        let line = group.apply_command_result(operation, &member, result, value);
        if operation == party_operation::LEAVE && result == party_result::OK {
            removal_end(&mut group, &mut errors, before, map.kind());
            summon.cancel();
        }
        // By key, so the catalog row picks the surface; `None` is the reference's silence.
        errors.0.extend(line);
    }
}

fn on_member_stats(In(ev): In<SessionEvent>, mut group: ResMut<GroupState>) {
    if let SessionEvent::PartyMemberStats { guid, full, info } = ev {
        group.apply_stats(guid, full, *info);
    }
}

/// `MSG_RAID_TARGET_UPDATE` (`0x5743b0`). After a one-mark update naming its setter, the last pair's
/// target, when nonzero, gets the chat notice (`0x5744cf`): only for a target we hold, and only
/// with the setter's name cached, never queried (`0x50b0f0`).
fn on_raid_target(
    In(ev): In<SessionEvent>,
    mut group: ResMut<GroupState>,
    mut notices: ResMut<GroupNotices>,
    names: Res<NameCache>,
    index: Res<GuidIndex>,
    stores: Query<&ObjectStore>,
    commands: Res<NetCommands>,
) {
    match ev {
        SessionEvent::RaidTargetSet { setter, marks } => {
            group.apply_raid_targets(&marks);
            let Some(&(icon, target)) = marks.last() else {
                return;
            };
            if setter == 0 || target == 0 {
                return;
            }
            let Some(store) = index.0.get(&target).and_then(|e| stores.get(*e).ok()) else {
                return;
            };
            let Some(setter_name) = names.peek(setter) else {
                return;
            };
            let target_name = names
                .resolve_unit(target, Some(store), &commands)
                .map(str::to_string);
            notices.target_icons.push(TargetIconNotice {
                setter: setter_name.to_string(),
                icon,
                target: target_name,
            });
        }
        SessionEvent::RaidTargetList { entries } => group.apply_raid_target_list(&entries),
        _ => {}
    }
}

/// The three ready-check packets, queued in order for the engine's port of their handlers.
fn on_ready_check(In(ev): In<SessionEvent>, mut group: ResMut<GroupState>) {
    use super::ReadyCheckWire as W;
    let wire = match ev {
        SessionEvent::ReadyCheckStarted { initiator } => W::Started { initiator },
        SessionEvent::ReadyCheckConfirm { guid, ready } => W::Confirm { guid, ready },
        SessionEvent::ReadyCheckFinished => W::Finished,
        _ => return,
    };
    group.apply_ready_check(wire);
}

fn on_raid_instance_info(In(ev): In<SessionEvent>, mut group: ResMut<GroupState>) {
    if let SessionEvent::RaidInstanceInfo { entries } = ev {
        group.apply_raid_instance_info(entries);
    }
}

/// The group state dies with the socket, after the bridge's own teardown.
fn on_session_end(In(_): In<SessionEvent>, mut group: ResMut<GroupState>) {
    group.clear_session();
}

/// Queue the messages an `apply_*` named; `ui_action::feed_actions` resolves and shows them.
fn push_group_lines(errors: &mut UiErrorKeys, lines: Vec<UiError>) {
    errors.0.extend(lines);
}

/// We lead when our guid is the leader's, the reference's test.
fn invited(group: &mut GroupState, errors: &mut UiErrorKeys, can_accept: bool, inviter: &str) {
    push_group_lines(errors, group.apply_invited(can_accept, inviter));
}

fn declined(group: &mut GroupState, errors: &mut UiErrorKeys, name: &str) {
    push_group_lines(errors, group.apply_declined(name));
}

fn destroyed(group: &mut GroupState, errors: &mut UiErrorKeys) {
    push_group_lines(errors, group.apply_destroyed());
}

/// Our own name, cached from login (`session::connected`), picks the line; this never queries.
fn leader_changed(
    group: &mut GroupState,
    errors: &mut UiErrorKeys,
    name: &str,
    self_guid: &SelfGuid,
    names: &NameCache,
    net_commands: &NetCommands,
) {
    let own = self_guid
        .0
        .and_then(|g| names.resolve(g, net_commands).map(str::to_string));
    push_group_lines(errors, group.apply_leader_changed(name, own.as_deref()));
}

/// `SMSG_GROUP_LIST`: apply the roster, seat new members' records and re-ask the questgiver sweep
/// (shared-quest availability follows the roster). Every member is name-queried once: the answer's
/// race, class and gender are the only source of them for a member we never see streamed. A raid's
/// rebuild counts the rows whose name is still to come, ours among them once others are listed, and
/// signals `RAID_ROSTER_UPDATE` only with none.
fn list(
    group: &mut GroupState,
    errors: &mut UiErrorKeys,
    sounds: &mut MessageSounds,
    quest: &mut QuestGiver,
    group_type: u8,
    own_flags: u8,
    members: Vec<GroupMemberEntry>,
    leader: u64,
    loot: Option<GroupLootInfo>,
    self_guid: &SelfGuid,
    names: &NameCache,
    index: &GuidIndex,
    net_commands: &NetCommands,
) {
    let mut names_pending = false;
    for m in &members {
        names_pending |= names.resolve(m.guid, net_commands).is_none();
    }
    if group_type == GROUPTYPE_RAID && !members.is_empty() {
        if let Some(own) = self_guid.0 {
            names_pending |= names.resolve(own, net_commands).is_none();
        }
    }
    // A party list with members onto no held group asks for the icon board (`0x6d9091`-`0x6d90d6`);
    // held is a party slot or a raid roster, as the list finds them (`0x6d8b21`, `0x6d8b53`).
    let ask_raid_targets = group_type != GROUPTYPE_RAID
        && !members.is_empty()
        && !(group.in_party() || group.in_raid());
    // Taken before `apply_list` consumes the list; `seat_new_records` then treats a member with
    // no record as new, the reference's `srcRec == 0`.
    let seats: Vec<(u64, bool)> = members
        .iter()
        .map(|m| (m.guid, m.listed_online()))
        .collect();
    let shown = group.apply_list_awaiting(
        group_type,
        own_flags,
        members,
        leader,
        loot,
        self_guid.0,
        names_pending,
    );
    push_group_lines(errors, shown.lines);
    if shown.invite_accept {
        sounds.push_cue(INVITE_ACCEPT_SOUND);
    }
    if ask_raid_targets {
        let _ = net_commands.0.send(ClientCommand::RequestRaidTargets);
    }
    seat_new_records(group, &seats, index, net_commands);
    quest.bump_reask();
}

/// `SMSG_GROUP_LIST`'s record leg: a known member's record carries over, its online bit rewritten
/// by [`GroupState::apply_list`], and nothing is sent; a new member gets the 1/1 placeholder and,
/// when we hold no object for them, a stats request. A listed member therefore owns a record, so an
/// unseen one shows full bars, not 0/0, until their stats land.
fn seat_new_records(
    group: &mut GroupState,
    seats: &[(u64, bool)],
    index: &GuidIndex,
    net_commands: &NetCommands,
) {
    for (guid, online) in seats {
        // `apply_list` kept only the staying members' records, so a missing one means new.
        if group.stats.contains_key(guid) {
            continue;
        }
        group
            .stats
            .insert(*guid, PartyMemberStatsInfo::placeholder(*online));
        if !index.0.contains_key(guid) {
            let _ = net_commands
                .0
                .send(ClientCommand::RequestPartyMemberStats { guid: *guid });
        }
    }
}

/// The despawn hook (object destroyed or out of range): for a roster member, fill their record from
/// the live descriptor and aura table (`0x6e12e0`), then request their stats. Nothing happens off
/// the roster or without an object, since the hook is a virtual on the object.
pub(crate) fn member_deactivated(
    guid: u64,
    group: &mut GroupState,
    held: Option<(&ObjectStore, Option<&UnitAuras>)>,
    net_commands: &NetCommands,
) {
    let Some((store, auras)) = held else {
        return;
    };
    if !group.members.iter().any(|m| m.guid == guid) {
        return;
    }
    let auras = auras
        .into_iter()
        .flat_map(UnitAuras::iter)
        .map(|a| (a.spell_id, a.flags));
    group
        .stats
        .entry(guid)
        .or_default()
        .snapshot_descriptor(&store.0, auras);
    let _ = net_commands
        .0
        .send(ClientCommand::RequestPartyMemberStats { guid });
}

/// [`member_deactivated`] for every streamed roster member: a cross-map transfer's teardown, where
/// the reference destroys each object and runs the hook on each.
pub(crate) fn roster_deactivated(
    group: &mut GroupState,
    index: &GuidIndex,
    stores: &Query<&mut ObjectStore>,
    auras: &Query<&UnitAuras>,
    net_commands: &NetCommands,
) {
    let streamed: Vec<u64> = group
        .members
        .iter()
        .map(|m| m.guid)
        .filter(|g| index.0.contains_key(g))
        .collect();
    for guid in streamed {
        member_deactivated(
            guid,
            group,
            held_member(index, stores, auras, guid),
            net_commands,
        );
    }
}

/// The streamed object of `guid` and its aura table, as [`member_deactivated`] takes them.
pub(crate) fn held_member<'a>(
    index: &GuidIndex,
    stores: &'a Query<&mut ObjectStore>,
    auras: &'a Query<&UnitAuras>,
    guid: u64,
) -> Option<(&'a ObjectStore, Option<&'a UnitAuras>)> {
    let &entity = index.0.get(&guid)?;
    Some((stores.get(entity).ok()?, auras.get(entity).ok()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::ClientCommand;
    use benilla_protocol::guid;
    use benilla_protocol::messages::member_status;

    fn member(g: u64, name: &str) -> GroupMemberEntry {
        GroupMemberEntry {
            name: name.into(),
            guid: g,
            status: 1, // ONLINE
            flags: 0,
            roles: 0,
        }
    }

    #[test]
    fn the_roster_warms_every_member_into_the_name_cache_once() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let net = NetCommands(tx);
        let (mut group, mut errors, mut quest) = (
            GroupState::default(),
            UiErrorKeys::default(),
            QuestGiver::default(),
        );
        let mut names = NameCache::default();
        // A player guid, `counter | (high << 48)`, the shape `NameCache::resolve` routes on.
        let player_guid = |counter: u64| counter | (u64::from(guid::HIGH_PLAYER) << 48);
        let (leader, far) = (player_guid(7), player_guid(8));

        list(
            &mut group,
            &mut errors,
            &mut MessageSounds::default(),
            &mut quest,
            0,
            0,
            vec![member(leader, "Aldwyn"), member(far, "Brisca")],
            leader,
            None,
            &SelfGuid::default(),
            &names,
            &GuidIndex::default(),
            &net,
        );

        let asked: Vec<u64> = rx
            .try_iter()
            .filter_map(|c| match c {
                ClientCommand::NameQuery { guid } => Some(guid),
                _ => None,
            })
            .collect();
        assert_eq!(
            asked,
            vec![leader, far],
            "both members asked, in roster order"
        );

        // The answer lands for one of them; the re-sent roster asks for neither.
        names.insert_player(leader, "Aldwyn".into(), Some((1, 4, 1)));
        list(
            &mut group,
            &mut errors,
            &mut MessageSounds::default(),
            &mut quest,
            0,
            0,
            vec![member(leader, "Aldwyn"), member(far, "Brisca")],
            leader,
            None,
            &SelfGuid::default(),
            &names,
            &GuidIndex::default(),
            &net,
        );
        assert!(
            rx.try_iter()
                .all(|c| !matches!(c, ClientCommand::NameQuery { .. })),
            "a re-sent roster re-asks nothing"
        );
        assert_eq!(names.player_traits(leader), Some((1, 4, 1)));
    }

    /// The rebuild counts the rows whose name has not landed, ours among them once others are
    /// listed, and the roster event waits for none.
    #[test]
    fn a_raid_list_signals_the_roster_only_once_every_name_is_held() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let net = NetCommands(tx);
        let (mut group, mut errors, mut quest) = (
            GroupState::default(),
            UiErrorKeys::default(),
            QuestGiver::default(),
        );
        let mut names = NameCache::default();
        let player_guid = |counter: u64| counter | (u64::from(guid::HIGH_PLAYER) << 48);
        let (me, a, b) = (player_guid(1), player_guid(7), player_guid(8));
        let mut raid = |group: &mut GroupState, names: &NameCache, group_type: u8| {
            list(
                group,
                &mut errors,
                &mut MessageSounds::default(),
                &mut quest,
                group_type,
                0,
                vec![member(a, "Aldwyn"), member(b, "Brisca")],
                a,
                None,
                &SelfGuid(Some(me)),
                names,
                &GuidIndex::default(),
                &net,
            );
            group.roster_updates
        };

        assert_eq!(raid(&mut group, &names, 1), 0, "no name held");
        names.insert_player(a, "Aldwyn".into(), Some((1, 4, 1)));
        assert_eq!(
            raid(&mut group, &names, 1),
            0,
            "one member's name still to come"
        );
        names.insert_player(b, "Brisca".into(), Some((1, 5, 0)));
        assert_eq!(
            raid(&mut group, &names, 1),
            0,
            "our own row's name still to come"
        );
        names.insert_player(me, "Me".into(), None);
        assert_eq!(raid(&mut group, &names, 1), 1, "every name held");
        assert_eq!(raid(&mut group, &names, 1), 2, "and the same list again");

        // A party's list runs no roster writer; it names nothing, and drops the raid it finds.
        assert_eq!(
            raid(&mut group, &NameCache::default(), 0),
            3,
            "the raid dropped"
        );
        assert_eq!(
            raid(&mut group, &NameCache::default(), 0),
            3,
            "a party's list"
        );
    }

    #[test]
    fn a_join_queues_the_invite_accept_chime_and_a_resync_does_not() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let net = NetCommands(tx);
        let (mut group, mut errors, mut sounds, mut quest) = (
            GroupState::default(),
            UiErrorKeys::default(),
            MessageSounds::default(),
            QuestGiver::default(),
        );
        let names = NameCache::default();
        let mut send = |sounds: &mut MessageSounds, members: Vec<GroupMemberEntry>| {
            list(
                &mut group,
                &mut errors,
                sounds,
                &mut quest,
                0,
                0,
                members,
                7,
                None,
                &SelfGuid(Some(9)),
                &names,
                &GuidIndex::default(),
                &net,
            );
        };

        send(&mut sounds, vec![member(7, "Aldwyn")]);
        assert_eq!(
            sounds.queued_cues(),
            ["igPlayerInviteAccept"],
            "our first party"
        );
        send(&mut sounds, vec![member(7, "Aldwyn")]);
        assert_eq!(sounds.queued_cues().len(), 1, "a resync is silent");
        send(&mut sounds, vec![member(7, "Aldwyn"), member(8, "Brisca")]);
        assert_eq!(sounds.queued_cues().len(), 2, "a member joining our party");
        assert_eq!(
            benilla_ui::messages::by_key("ERR_JOINED_GROUP_S").and_then(|r| r.sound),
            None,
            "the join line's own row carries no sound, so the chime is the only one"
        );
    }

    // ── The out-of-range record ──────────────────────────────────────────────

    /// Unit descriptor field indices, build 5875.
    const HEALTH: u16 = 22;
    const MAXHEALTH: u16 = 28;
    /// `UNIT_FIELD_POWER2`/`MAXPOWER2`, the rage slot (`POWER1 + POWER_RAGE`).
    const POWER2: u16 = 24;
    const MAXPOWER2: u16 = 30;
    const LEVEL: u16 = 34;
    const BYTES_0: u16 = 36;

    fn asked(rx: &crossbeam_channel::Receiver<ClientCommand>) -> Vec<u64> {
        rx.try_iter()
            .filter_map(|c| match c {
                ClientCommand::RequestPartyMemberStats { guid } => Some(guid),
                _ => None,
            })
            .collect()
    }

    fn grouped(members: &[GroupMemberEntry]) -> GroupState {
        let mut group = GroupState::default();
        group.apply_list(0, 0, members.to_vec(), members[0].guid, None, None);
        group
    }

    #[test]
    fn a_members_despawn_snapshots_their_descriptor_and_asks_for_their_stats() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let net = NetCommands(tx);
        let guid = 0x1234;
        let mut group = grouped(&[member(guid, "Brisca"), member(0x99, "Aldwyn")]);
        let _ = asked(&rx); // start from an empty queue

        // A warrior: rage is stored raw, ten times the shown value, as in the descriptor.
        let store = ObjectStore(benilla_protocol::messages::ObjectFields::from_pairs(&[
            (HEALTH, 2400),
            (MAXHEALTH, 3000),
            (POWER2, 570),
            (MAXPOWER2, 1000),
            (LEVEL, 41),
            (BYTES_0, 1 << 24), // POWER_RAGE in BYTES_0 byte 3
        ]));
        member_deactivated(guid, &mut group, Some((&store, None)), &net);

        let rec = group.stats.get(&guid).expect("the member has a record");
        assert_eq!(
            (rec.cur_hp, rec.max_hp, rec.level),
            (Some(2400), Some(3000), Some(41)),
            "the bars keep the numbers they were showing at the edge"
        );
        assert_eq!(
            (rec.power_type, rec.cur_power, rec.max_power),
            (Some(1), Some(570), Some(1000)),
            "RAW power, like the reference's record — the ÷10 happens at the read"
        );
        assert_eq!((rec.shown_power(), rec.shown_max_power()), (57, 100));
        assert_eq!(
            rec.status,
            Some(member_status::ONLINE.into()),
            "an object you can see belongs to an online, living, unflagged player"
        );
        assert_eq!(asked(&rx), vec![guid], "and the server is asked, once");
    }

    /// `0x6e14a8`-`0x6e14fa`: an aura is copied when its id is set and its flags have an effect
    /// bit, packed from slot 0 with its flags; of the pet block only the guid is written
    /// (`0x6e14fc`-`0x6e151e`), and the status takes the AFK and DND player flags (`0x6e1354`).
    #[test]
    fn a_members_despawn_snapshots_their_auras_and_only_their_pets_guid() {
        const SUMMON: u16 = 8;
        const PLAYER_FLAGS: u16 = 190;
        let (tx, _rx) = crossbeam_channel::unbounded();
        let net = NetCommands(tx);
        let guid = 0x1234;
        let pet_guid = 0xF140_0000_0000_0077;
        let mut group = grouped(&[member(guid, "Brisca")]);
        group.stats.insert(
            guid,
            PartyMemberStatsInfo {
                pet_name: Some("Whelp".into()),
                pet_cur_hp: Some(900),
                ..PartyMemberStatsInfo::default()
            },
        );

        let store = ObjectStore(benilla_protocol::messages::ObjectFields::from_pairs(&[
            (HEALTH, 2400),
            (SUMMON, pet_guid as u32),
            (SUMMON + 1, (pet_guid >> 32) as u32),
            (PLAYER_FLAGS, 0x2 | 0x4),
        ]));
        let auras = UnitAuras::of(
            0x99,
            &[
                (0, 1126, 0x19), // live, helpful
                (1, 6673, 0x10), // no effect bit
                (5, 589, 0x81),  // live, harmful
            ],
        );
        member_deactivated(guid, &mut group, Some((&store, Some(&auras))), &net);

        let rec = group.stats.get(&guid).expect("the member has a record");
        let aura = |slot, spell_id, flags| benilla_protocol::messages::RecordAura {
            slot,
            spell_id,
            flags,
        };
        assert_eq!(
            rec.auras,
            Some(vec![aura(0, 1126, 0x19), aura(1, 589, 0x81)]),
            "the effectless aura is dropped and the rest packed from slot 0"
        );
        assert_eq!(rec.pet_guid, Some(pet_guid));
        assert_eq!(
            (rec.pet_name.as_deref(), rec.pet_cur_hp),
            (Some("Whelp"), Some(900)),
            "the rest of the pet block is left as it was"
        );
        assert_eq!(
            rec.status,
            Some(u16::from(
                member_status::ONLINE | member_status::AFK | member_status::DND
            ))
        );

        // No aura table and no pet: an empty block and no pet guid.
        let bare = ObjectStore(benilla_protocol::messages::ObjectFields::from_pairs(&[(
            HEALTH, 2400,
        )]));
        member_deactivated(guid, &mut group, Some((&bare, None)), &net);
        let rec = group.stats.get(&guid).unwrap();
        assert_eq!((rec.pet_guid, rec.auras.clone()), (None, Some(vec![])));
    }

    #[test]
    fn a_despawn_that_is_not_a_party_member_asks_nothing() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let net = NetCommands(tx);
        let mut group = grouped(&[member(0x1234, "Brisca")]);
        let _ = asked(&rx);
        // A live object, so only the roster gate can stop it.
        let store = ObjectStore(benilla_protocol::messages::ObjectFields::from_pairs(&[(
            HEALTH, 40,
        )]));
        member_deactivated(0xdead, &mut group, Some((&store, None)), &net);
        assert!(asked(&rx).is_empty());
        assert!(!group.stats.contains_key(&0xdead));

        // The object gate alone: a roster member we hold no object for asks nothing.
        member_deactivated(0x1234, &mut group, None, &net);
        assert!(asked(&rx).is_empty());
    }

    #[test]
    fn a_roster_new_member_is_seated_at_one_one_and_asked_for_only_when_unseen() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let net = NetCommands(tx);
        let (mut group, mut errors, mut quest) = (
            GroupState::default(),
            UiErrorKeys::default(),
            QuestGiver::default(),
        );
        let names = NameCache::default();
        let (near, far) = (0x11u64, 0x22u64);

        // `near` is streamed, `far` is not.
        let mut index = GuidIndex::default();
        let mut world = bevy::ecs::world::World::new();
        index.0.insert(near, world.spawn_empty().id());

        let roster = vec![member(near, "Aldwyn"), member(far, "Brisca")];
        let mut send = |group: &mut GroupState, members: Vec<GroupMemberEntry>| {
            list(
                group,
                &mut errors,
                &mut MessageSounds::default(),
                &mut quest,
                0,
                0,
                members,
                near,
                None,
                &SelfGuid::default(),
                &names,
                &index,
                &net,
            );
        };

        send(&mut group, roster.clone());
        assert_eq!(
            asked(&rx),
            vec![far],
            "only the member whose object we do not hold"
        );
        for guid in [near, far] {
            let rec = group.stats.get(&guid).expect("every member owns a record");
            assert_eq!(
                (rec.cur_hp, rec.max_hp, rec.cur_power, rec.max_power),
                (Some(1), Some(1), Some(1), Some(1)),
                "a full bar, not an empty one, until the stats land"
            );
        }

        // The wire answers for `far`, then the roster is re-sent: nothing is asked again.
        group.apply_stats(
            far,
            true,
            PartyMemberStatsInfo {
                cur_hp: Some(900),
                max_hp: Some(1100),
                ..PartyMemberStatsInfo::default()
            },
        );
        send(&mut group, roster);
        assert!(
            asked(&rx).is_empty(),
            "a resync of a known roster asks nothing"
        );
        assert_eq!(
            group.stats.get(&far).map(|r| (r.cur_hp, r.max_hp)),
            Some((Some(900), Some(1100))),
            "and the record it already had survives the resync"
        );
    }

    /// A re-sent list that moves only a member's status rewrites the online bit of the record the
    /// member already owns, asks nothing, and leaves the rest of the record.
    #[test]
    fn a_resent_list_moves_a_known_members_online_bit_and_keeps_its_record() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let net = NetCommands(tx);
        let (mut group, mut errors, mut quest) = (
            GroupState::default(),
            UiErrorKeys::default(),
            QuestGiver::default(),
        );
        let names = NameCache::default();
        let far = 0x22u64;
        let mut send = |group: &mut GroupState, status: u8| {
            let row = GroupMemberEntry {
                status,
                ..member(far, "Brisca")
            };
            list(
                group,
                &mut errors,
                &mut MessageSounds::default(),
                &mut quest,
                0,
                0,
                vec![row],
                far,
                None,
                &SelfGuid::default(),
                &names,
                &GuidIndex::default(),
                &net,
            );
        };

        send(&mut group, member_status::ONLINE);
        group.apply_stats(
            far,
            false,
            PartyMemberStatsInfo {
                cur_hp: Some(900),
                pet_guid: Some(0xF140_0000_0000_0077),
                ..PartyMemberStatsInfo::default()
            },
        );
        assert!(group.stats[&far].is_online(), "seated from the row");
        let _ = asked(&rx);

        send(&mut group, member_status::OFFLINE);
        let rec = &group.stats[&far];
        assert!(!rec.is_online(), "the row's byte moved the record's bit");
        assert_eq!(
            (rec.cur_hp, rec.pet_guid),
            (Some(900), Some(0xF140_0000_0000_0077)),
            "and nothing else"
        );
        assert!(asked(&rx).is_empty(), "no stats request for a known member");

        send(&mut group, member_status::ONLINE);
        assert!(group.stats[&far].is_online());
    }

    // ── The packets' handlers ────────────────────────────────────────────────────────────

    fn handler_world() -> bevy::ecs::world::World {
        let mut world = bevy::ecs::world::World::new();
        world.init_resource::<GroupState>();
        world.init_resource::<GroupNotices>();
        world.init_resource::<UiErrorKeys>();
        world.init_resource::<SummonState>();
        world.init_resource::<NameCache>();
        world.init_resource::<GuidIndex>();
        let (tx, _rx) = crossbeam_channel::unbounded();
        world.insert_resource(NetCommands(tx));
        world
    }

    fn player_guid(counter: u64) -> u64 {
        counter | (u64::from(guid::HIGH_PLAYER) << 48)
    }

    fn command_result(operation: u32, result: u32) -> SessionEvent {
        SessionEvent::PartyCommandResult {
            operation,
            member: String::new(),
            result,
            value: 0,
        }
    }

    /// `0x6cbe2a`, `0x6cbe66`, `0x6cbf2e`: the kick, the disband and our leave's ack each owe
    /// `CANCEL_SUMMON`; another command result does not.
    #[test]
    fn the_three_ways_out_of_a_group_owe_cancel_summon() {
        use bevy::ecs::system::RunSystemOnce;
        let mut world = handler_world();
        world
            .run_system_once_with(on_uninvited, SessionEvent::GroupUninvited)
            .unwrap();
        world
            .run_system_once_with(on_destroyed, SessionEvent::GroupDestroyed)
            .unwrap();
        for (operation, result) in [
            (party_operation::LEAVE, party_result::OK),
            (party_operation::INVITE, party_result::OK),
            (party_operation::LEAVE, party_result::NOT_IN_GROUP),
        ] {
            world
                .run_system_once_with(on_command_result, command_result(operation, result))
                .unwrap();
        }
        assert_eq!(world.resource::<SummonState>().cancels_owed(), 3);
    }

    /// `0x6cbf14`: our leave's ack runs the party clear-all, whose difficulty line comes first, and
    /// then shows `ERR_LEFT_GROUP_YOU`.
    #[test]
    fn the_leave_ack_prints_the_difficulty_change_before_its_own_line() {
        use bevy::ecs::system::RunSystemOnce;
        let mut world = handler_world();
        {
            let mut g = world.resource_mut::<GroupState>();
            g.apply_list(0, 0, vec![member(0x11, "Aldwyn")], 0x11, None, None);
            g.difficulties.dungeon = 1;
        }
        let leave = command_result(party_operation::LEAVE, party_result::OK);
        world
            .run_system_once_with(on_command_result, leave)
            .unwrap();
        let keys: Vec<&str> = world
            .resource::<UiErrorKeys>()
            .0
            .iter()
            .map(|e| e.key)
            .collect();
        assert_eq!(
            keys,
            ["ERR_DUNGEON_DIFFICULTY_CHANGED_S", "ERR_LEFT_GROUP_YOU"]
        );
    }

    /// `0x6cbd10`: the withdrawn invite clears the popup's inviter, which the invite edge answers
    /// with `PARTY_INVITE_CANCEL`.
    #[test]
    fn a_withdrawn_invite_clears_the_pending_inviter() {
        use bevy::ecs::system::RunSystemOnce;
        let mut world = handler_world();
        world
            .resource_mut::<GroupState>()
            .apply_invited(true, "Bob");
        let cancel = SessionEvent::GroupCancel {
            inviter: "Bob".into(),
        };
        world.run_system_once_with(on_cancel, cancel).unwrap();
        assert_eq!(world.resource::<GroupState>().pending_invite, None);
    }

    /// `0x5744cf`, `0x50b0f0`: a one-mark update owes the notice for a cached setter and a held,
    /// nonzero target only, and every packet moves the board.
    #[test]
    fn a_one_mark_update_owes_the_notice_only_for_a_named_setter_and_a_held_target() {
        use bevy::ecs::system::RunSystemOnce;
        let mut world = handler_world();
        let (setter, target, stranger, away) = (
            player_guid(1),
            player_guid(2),
            player_guid(3),
            player_guid(4),
        );
        {
            let mut names = world.resource_mut::<NameCache>();
            names.insert_player(setter, "Aldwyn".into(), None);
            names.insert_player(target, "Brisca".into(), None);
        }
        let held = world.spawn(ObjectStore::default()).id();
        world.resource_mut::<GuidIndex>().0.insert(target, held);
        let mark = |setter, marks: Vec<(u8, u64)>| SessionEvent::RaidTargetSet { setter, marks };
        for ev in [
            mark(setter, vec![(7, target)]),
            mark(stranger, vec![(6, target)]),
            mark(setter, vec![(5, away)]),
            mark(setter, vec![(4, 0)]),
            mark(0, vec![(3, target)]),
            mark(setter, vec![]),
        ] {
            world.run_system_once_with(on_raid_target, ev).unwrap();
        }
        let notices = &world.resource::<GroupNotices>().target_icons;
        assert_eq!(notices.len(), 1, "the named setter's mark on a held unit");
        let n = &notices[0];
        assert_eq!(
            (n.setter.as_str(), n.icon, n.target.as_deref()),
            ("Aldwyn", 7, Some("Brisca"))
        );
        let g = world.resource::<GroupState>();
        assert_eq!(g.raid_targets[3], target);
        assert_eq!(g.raid_target_updates, 6, "every packet signals");
    }

    /// `0x6cbe80`, `0x5725a0`, `0x5725c0`: one line each; `0x57264c`: the echo names a cached
    /// member and shows nothing for an uncached one.
    #[test]
    fn the_notices_show_their_lines_and_the_echo_needs_a_cached_name() {
        use bevy::ecs::system::RunSystemOnce;
        let mut world = handler_world();
        let member = player_guid(7);
        world
            .resource_mut::<NameCache>()
            .insert_player(member, "Brisca".into(), None);
        for ev in [
            SessionEvent::GroupActionThrottled,
            SessionEvent::ReadyCheckError,
            SessionEvent::PartySquelch { silenced: true },
            SessionEvent::PartySquelch { silenced: false },
        ] {
            world.run_system_once_with(on_keyed_notice, ev).unwrap();
        }
        for (guid, silenced) in [(member, true), (player_guid(8), true), (member, false)] {
            let ev = SessionEvent::EchoPartySquelch { guid, silenced };
            world.run_system_once_with(on_echo_squelch, ev).unwrap();
        }
        assert_eq!(
            world.resource::<UiErrorKeys>().0,
            vec![
                UiError::key("ERR_GROUP_ACTION_THROTTLED"),
                UiError::key("ERR_READY_CHECK_THROTTLED"),
                UiError::key("ERR_PLAYER_SILENCED"),
                UiError::key("ERR_PLAYER_UNSILENCED"),
                UiError::s("ERR_PLAYER_SILENCED_ECHO", "Brisca"),
                UiError::s("ERR_PLAYER_UNSILENCED_ECHO", "Brisca"),
            ]
        );
    }

    /// `0x6d9091`-`0x6d90d6`: a party list with members onto no held group asks for the icon
    /// board; a list onto a held group, a raid's list and an empty list do not.
    #[test]
    fn joining_a_party_asks_for_the_raid_target_board() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let net = NetCommands(tx);
        let (mut group, mut errors, mut quest) = (
            GroupState::default(),
            UiErrorKeys::default(),
            QuestGiver::default(),
        );
        let names = NameCache::default();
        let mut send = |group: &mut GroupState, group_type, members, leader| {
            list(
                group,
                &mut errors,
                &mut MessageSounds::default(),
                &mut quest,
                group_type,
                0,
                members,
                leader,
                None,
                &SelfGuid(Some(9)),
                &names,
                &GuidIndex::default(),
                &net,
            );
            rx.try_iter()
                .filter(|c| matches!(c, ClientCommand::RequestRaidTargets))
                .count()
        };
        let party = || vec![member(7, "Aldwyn")];
        assert_eq!(send(&mut group, 0, party(), 7), 1, "joining a party");
        assert_eq!(send(&mut group, 0, party(), 7), 0, "a resync");
        assert_eq!(send(&mut group, 0, vec![], 0), 0, "leaving");
        assert_eq!(send(&mut group, 1, party(), 7), 0, "joining a raid");
        assert_eq!(send(&mut group, 0, vec![], 0), 0);
        assert_eq!(send(&mut group, 0, vec![], 9), 0, "a list of nobody");
    }
}
