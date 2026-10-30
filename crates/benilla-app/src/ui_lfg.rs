//! The dungeon finder feed: the five tables pushed once per VM, every `SMSG_LFG_*` handed to the
//! VM's port of the reference's dungeon-finder engine in wire order, the group edges it reacts to,
//! and the sends its verbs queue. A packet waits in [`LfgFeed`] until a VM is up, so one landing
//! between the login and the UI's first frame is applied in order after the VM's game
//! initialization.

use std::collections::HashMap;
use std::sync::Arc;

use bevy::prelude::*;

use benilla_assets::{AssetSet, LockRecover, WorldAssets};
use benilla_protocol::messages::{
    LfgCompletionReward, LfgMemberLocks, LfgPacket, LfgRewardItem, LfgUpdate,
};
use benilla_protocol::{SessionEvent, SessionEventKind};
use benilla_ui::script::{
    EncounterView, LfgBootView, LfgCatalogView, LfgCompletionRewardView, LfgDungeonRewardView,
    LfgDungeonView, LfgExpansionView, LfgGroupView, LfgLine, LfgMemberLocksView, LfgName,
    LfgPartyView, LfgProposalMemberView, LfgProposalView, LfgQueueStatusView, LfgRewardItemView,
    LfgRoleCheckMemberView, LfgRoleCheckView, LfgUpdateView, MapDifficultyView, UiScript,
};

use crate::char_select::ClientState;
use crate::chr_classes::ChrClassTable;
use crate::entities::ItemDisplays;
use crate::names::NameCache;
use crate::net::WorldEnterCascadeMessage;
use crate::net::{ClientCommand, NetCommands, NetHandlerApp, ObjectStore, SelfGuid, SelfPlayer};
use crate::sound::MessageSounds;
use crate::ui_action::{FillArg, UiError, UiErrorKeys};
use crate::ui_party::GroupState;
use crate::ui_script::{UiFeed, VmMemo};

/// The five tables, absent when the client data failed to load.
#[derive(Resource, Clone)]
pub(crate) struct LfgCatalog(pub(crate) Arc<LfgCatalogView>);

/// What waits for the VM, in wire order.
enum Pending {
    Packet { packet: LfgPacket, grouped: bool },
    JoiningParty,
    ClearPartyInfo,
    EnterWorld,
}

/// The feed's session state: the waiting work and the group and instance facts the packets read.
#[derive(Resource, Default)]
pub(crate) struct LfgFeed {
    pending: Vec<Pending>,
    party: LfgPartyView,
    /// `[0xbd0894]` and the dynamic difficulty, from `SMSG_INSTANCE_DIFFICULTY`.
    difficulty: (u32, u32),
}

pub(crate) struct UiLfgPlugin;

impl Plugin for UiLfgPlugin {
    fn build(&self, app: &mut App) {
        use SessionEventKind as K;
        app.init_resource::<LfgFeed>()
            .add_systems(Startup, load_lfg_catalog.after(AssetSet::Open))
            .add_systems(
                Update,
                (
                    lfg_enter_world
                        .in_set(UiFeed)
                        .after(crate::ui_battlefield::reset_on_world_enter),
                    // Gated, so a frameless boot VM never takes a packet the in-game VM needs.
                    // After a map change's cache eviction, which writes the icon catalog's holder.
                    feed_lfg
                        .in_set(UiFeed)
                        .after(lfg_enter_world)
                        .after(crate::entities::DisplayEvictSet)
                        .run_if(crate::ui_script::ingame_ui_up),
                ),
            )
            .add_systems(OnExit(ClientState::InWorld), end_session)
            .net_handler(K::Lfg, on_lfg)
            .net_handler(K::GroupList, on_group_list)
            .net_handler(K::GroupUninvited, on_group_left)
            .net_handler(K::GroupDestroyed, on_group_left)
            .net_handler(K::PartyCommandResult, on_group_left)
            .net_handler(K::InstanceDifficulty, on_instance_difficulty);
    }
}

fn load_lfg_catalog(mut commands: Commands, assets: Option<Res<WorldAssets>>) {
    let Some(assets) = assets else { return };
    let loaded = read_catalog(&mut assets.chain.lock_recover());
    match loaded {
        Ok(cat) => {
            info!(
                "ui_lfg: {} dungeon(s), {} group(s), {} difficulty row(s), {} encounter(s)",
                cat.dungeons.len(),
                cat.groups.len(),
                cat.map_difficulties.len(),
                cat.encounters.len()
            );
            commands.insert_resource(LfgCatalog(Arc::new(cat)));
        }
        Err(e) => {
            warn!("ui_lfg: the dungeon finder tables failed to load, the window stays empty: {e:#}")
        }
    }
}

/// The five tables off the patch chain, as the engine takes them.
pub(crate) fn read_catalog(chain: &mut benilla_formats::Chain) -> anyhow::Result<LfgCatalogView> {
    let lfg = benilla_formats::load_lfg_catalog(chain)?;
    let difficulties = benilla_formats::load_map_difficulties(chain)?;
    let encounters = benilla_formats::load_dungeon_encounters(chain)?;
    Ok(catalog_of(lfg, difficulties, encounters))
}

/// The formats crate's rows as the script engine's.
fn catalog_of(
    lfg: benilla_formats::LfgCatalog,
    difficulties: Vec<benilla_formats::MapDifficulty>,
    encounters: Vec<benilla_formats::DungeonEncounter>,
) -> LfgCatalogView {
    LfgCatalogView {
        dungeons: lfg
            .dungeons
            .into_iter()
            .map(|d| LfgDungeonView {
                id: d.id,
                name: d.name,
                min_level: d.min_level,
                max_level: d.max_level,
                target_level: d.target_level,
                target_level_min: d.target_level_min,
                target_level_max: d.target_level_max,
                map_id: d.map_id,
                difficulty: d.difficulty,
                flags: d.flags,
                type_id: d.type_id,
                faction: d.faction,
                texture: d.texture,
                expansion_level: d.expansion_level,
                order_index: d.order_index,
                group_id: d.group_id,
                description: d.description,
            })
            .collect(),
        groups: lfg
            .groups
            .into_iter()
            .map(|g| LfgGroupView {
                id: g.id,
                name: g.name,
                order_index: g.order_index,
                parent_group_id: g.parent_group_id,
                type_id: g.type_id,
            })
            .collect(),
        expansions: lfg
            .expansions
            .into_iter()
            .map(|e| LfgExpansionView {
                lfg_id: e.lfg_id,
                expansion_level: e.expansion_level,
                hard_level_min: e.hard_level_min,
                hard_level_max: e.hard_level_max,
                target_level_min: e.target_level_min,
                target_level_max: e.target_level_max,
            })
            .collect(),
        map_difficulties: difficulties
            .into_iter()
            .map(|d| MapDifficultyView {
                map_id: d.map_id,
                difficulty: d.difficulty,
                max_players: d.max_players,
                difficulty_string: d.difficulty_string,
            })
            .collect(),
        encounters: encounters
            .into_iter()
            .map(|e| EncounterView {
                map_id: e.map_id,
                difficulty: e.difficulty,
                bit: e.bit,
                name: e.name,
                icon: e.icon,
            })
            .collect(),
    }
}

fn on_lfg(In(ev): In<SessionEvent>, mut feed: ResMut<LfgFeed>) {
    if let SessionEvent::Lfg(packet) = ev {
        match &packet {
            LfgPacket::UpdatePlayer(u) | LfgPacket::UpdateParty(u) => debug!(
                "ui_lfg: {} type {} joined {} queued {} slots {:?}",
                packet.name(),
                u.update_type,
                u.joined,
                u.queued,
                u.slots
            ),
            LfgPacket::Proposal(p) => debug!(
                "ui_lfg: {} slot {:#x} state {} id {} answers {:?}",
                packet.name(),
                p.slot,
                p.state,
                p.id,
                p.members
                    .iter()
                    .map(|m| (m.answered, m.accepted))
                    .collect::<Vec<_>>()
            ),
            _ => debug!("ui_lfg: {}", packet.name()),
        }
        let grouped = feed.party.grouped();
        feed.pending.push(Pending::Packet { packet, grouped });
    }
}

/// `SMSG_GROUP_LIST`'s LFG facts, read after the party feed applied the list; its join edge is
/// `JoiningParty` (`0x6d8870` → `0x559310`).
fn on_group_list(In(ev): In<SessionEvent>, mut feed: ResMut<LfgFeed>, group: Res<GroupState>) {
    let SessionEvent::GroupList {
        group_type,
        lfg_status,
        lfg_slot,
        ..
    } = ev
    else {
        return;
    };
    let raid = group_type == 1 && group.in_group;
    let party_members = group.party_slots().count() as u32;
    let next = LfgPartyView {
        in_party: group.in_party(),
        raid_members: if raid {
            group.members.len() as u32 + 1
        } else {
            0
        },
        party_members,
        real_members: party_members,
        lfg_slot,
        lfg_flags: lfg_status,
    };
    let was = feed.party;
    if (!was.grouped() && next.grouped()) || (was.raid_members == 0 && next.raid_members != 0) {
        feed.pending.push(Pending::JoiningParty);
    }
    feed.party = next;
}

/// The uninvite, the disband and our own leave's ack each end with `ClearPartyInfo`
/// (`0x6cbe10`, `0x6cbe40`, the command result's leave arm).
fn on_group_left(In(ev): In<SessionEvent>, mut feed: ResMut<LfgFeed>) {
    let left = match ev {
        SessionEvent::GroupUninvited | SessionEvent::GroupDestroyed => true,
        SessionEvent::PartyCommandResult {
            operation, result, ..
        } => operation == 2 && result == 0,
        _ => false,
    };
    if left {
        feed.party = LfgPartyView::default();
        feed.pending.push(Pending::ClearPartyInfo);
    }
}

fn on_instance_difficulty(In(ev): In<SessionEvent>, mut feed: ResMut<LfgFeed>) {
    if let SessionEvent::InstanceDifficulty {
        difficulty,
        dynamic,
    } = ev
    {
        debug!("ui_lfg: instance difficulty {difficulty}, dynamic {dynamic}");
        feed.difficulty = (difficulty, dynamic);
    }
}

/// The world-enter cascade's dungeon finder leg, after the battlefield status: `EnterWorld`
/// (`0x557240`) resets the proposal latch and asks the queue status and the locks.
pub(crate) fn lfg_enter_world(
    mut cascades: MessageReader<WorldEnterCascadeMessage>,
    mut feed: ResMut<LfgFeed>,
    commands: Res<NetCommands>,
) {
    if cascades.read().next().is_none() {
        return;
    }
    feed.pending.push(Pending::EnterWorld);
    use benilla_ui::script::LfgRequest as R;
    let _ = commands.0.send(ClientCommand::Lfg(R::GetStatus));
    let _ = commands.0.send(ClientCommand::Lfg(R::PlayerLockInfo));
}

fn end_session(mut feed: ResMut<LfgFeed>) {
    *feed = LfgFeed::default();
}

/// What the feed last pushed into this VM.
#[derive(Default)]
struct FeedMemory {
    names_generation: u64,
    names: HashMap<u64, LfgName>,
    icons: HashMap<u32, String>,
}

#[allow(clippy::too_many_arguments)]
fn feed_lfg(
    script: Option<NonSendMut<UiScript>>,
    catalog: Option<Res<LfgCatalog>>,
    mut feed: ResMut<LfgFeed>,
    current_map: Option<Res<benilla_world::world_map::CurrentMap>>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    self_guid: Res<SelfGuid>,
    names: Res<NameCache>,
    classes: Option<Res<ChrClassTable>>,
    displays: Option<Res<ItemDisplays>>,
    commands: Res<NetCommands>,
    mut errors: ResMut<UiErrorKeys>,
    mut sounds: ResMut<MessageSounds>,
    mut memory: Local<VmMemo<FeedMemory>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let (memory, vm_reset) = memory.get_reset(&script);
    if vm_reset {
        if let Some(c) = catalog.as_ref() {
            script.set_lfg_catalog((*c.0).clone());
        }
        script.lfg_initialize_game();
        memory.names_generation = u64::MAX;
    }
    if let Some(class) = self_q.single().ok().and_then(|s| s.0.unit_class()) {
        script.set_lfg_class(u32::from(class));
    }
    let map = current_map.map_or(-1, |m| m.0 as i32);
    script.set_lfg_world(map, feed.difficulty.0, feed.difficulty.1);
    script.set_lfg_party(feed.party);

    let me = self_guid.0.unwrap_or(0);
    let name_of = |guid: u64| names.resolve(guid, &commands).map(str::to_string);
    for item in std::mem::take(&mut feed.pending) {
        let lines = match item {
            Pending::EnterWorld => {
                script.lfg_enter_world();
                Vec::new()
            }
            Pending::JoiningParty => {
                script.lfg_joining_party();
                Vec::new()
            }
            Pending::ClearPartyInfo => {
                script.lfg_clear_party_info();
                Vec::new()
            }
            Pending::Packet { packet, grouped } => {
                apply(&mut script, packet, grouped, me, &name_of)
            }
        };
        show(&script, lines, &mut errors, &mut sounds);
    }

    for request in script.take_lfg_requests() {
        debug!("ui_lfg: send {request:?}");
        let _ = commands.0.send(ClientCommand::Lfg(request));
    }

    let wanted = script.lfg_wanted_names();
    if !wanted.is_empty() && (names.generation() != memory.names_generation || vm_reset) {
        memory.names_generation = names.generation();
        let pushed: HashMap<u64, LfgName> = wanted
            .into_iter()
            .filter_map(|guid| {
                let name = names.resolve(guid, &commands)?.to_string();
                let class_name = names
                    .player_traits(guid)
                    .and_then(|(_, class, _)| classes.as_ref()?.0.name(u32::from(class)))
                    .unwrap_or_default()
                    .to_string();
                Some((
                    guid,
                    LfgName {
                        name,
                        class_name,
                        realm: String::new(),
                    },
                ))
            })
            .collect();
        if pushed != memory.names {
            memory.names = pushed.clone();
            script.set_lfg_names(pushed);
        }
    }
    if let Some(displays) = displays {
        let icons: HashMap<u32, String> = script
            .lfg_wanted_displays()
            .into_iter()
            .filter_map(|d| Some((d, displays.catalog.get(d)?.icon.clone()?)))
            .collect();
        if icons != memory.icons {
            memory.icons = icons.clone();
            script.set_lfg_display_icons(icons);
        }
    }
}

/// One server message into the VM; what it shows comes back.
fn apply(
    script: &mut UiScript,
    packet: LfgPacket,
    grouped: bool,
    me: u64,
    name_of: &dyn Fn(u64) -> Option<String>,
) -> Vec<LfgLine> {
    match packet {
        LfgPacket::UpdatePlayer(u) => script.lfg_update_player(update(u), grouped),
        LfgPacket::UpdateParty(u) => script.lfg_update_party(update(u), grouped),
        // The raid browser's search list is not built; nothing is ever joined to end.
        LfgPacket::UpdateSearch { .. } => Vec::new(),
        LfgPacket::QueueStatus(s) => {
            script.lfg_queue_status(LfgQueueStatusView {
                dungeon: s.dungeon,
                my_wait: s.my_wait,
                average_wait: s.average_wait,
                tank_wait: s.tank_wait,
                healer_wait: s.healer_wait,
                damage_wait: s.damage_wait,
                tanks_needed: s.tanks_needed,
                healers_needed: s.healers_needed,
                damage_needed: s.damage_needed,
                queued_secs: s.queued_secs,
            });
            Vec::new()
        }
        LfgPacket::Disabled => script.lfg_disabled(),
        LfgPacket::Proposal(p) => script.lfg_proposal(LfgProposalView {
            slot: p.slot,
            state: p.state,
            id: p.id,
            completed_mask: p.completed_mask,
            silent: p.silent,
            members: p
                .members
                .iter()
                .map(|m| LfgProposalMemberView {
                    roles: m.roles,
                    is_local: m.is_local,
                    in_dungeon: m.in_dungeon,
                    same_group: m.same_group,
                    answered: m.answered,
                    accepted: m.accepted,
                })
                .collect(),
        }),
        LfgPacket::RoleCheck(rc) => script.lfg_role_check(
            LfgRoleCheckView {
                state: rc.state,
                beginning: rc.beginning,
                slots: rc.slots,
                members: rc
                    .members
                    .iter()
                    .map(|m| LfgRoleCheckMemberView {
                        guid: m.guid,
                        confirmed: m.confirmed,
                        roles: m.roles,
                        level: m.level,
                    })
                    .collect(),
            },
            me,
            name_of,
        ),
        LfgPacket::RoleChosen {
            guid,
            accepted,
            roles,
        } => script.lfg_role_chosen(name_of(guid), accepted, roles),
        LfgPacket::JoinResult {
            result,
            reason,
            party_locks,
        } => {
            debug!("ui_lfg: join result {result}, reason {reason}");
            script.lfg_join_result(result, reason, party_locks.map(locks))
        }
        LfgPacket::Boot(b) => {
            let target = name_of(b.target);
            script.lfg_boot(
                LfgBootView {
                    in_progress: b.in_progress,
                    did_vote: b.did_vote,
                    my_vote: b.my_vote,
                    target: b.target,
                    total_votes: b.total_votes,
                    boot_votes: b.boot_votes,
                    time_left: b.time_left,
                    votes_needed: b.votes_needed,
                    reason: b.reason,
                },
                target,
            )
        }
        LfgPacket::PlayerInfo { rewards, locks } => {
            script.lfg_player_info(
                rewards
                    .into_iter()
                    .map(|r| LfgDungeonRewardView {
                        slot: r.slot,
                        done: r.done,
                        base_money: r.base_money,
                        base_xp: r.base_xp,
                        variable_money: r.variable_money,
                        variable_xp: r.variable_xp,
                        items: items(&r.items),
                    })
                    .collect(),
                locks,
            );
            Vec::new()
        }
        LfgPacket::PartyInfo(members) => {
            script.lfg_party_info(locks(members));
            Vec::new()
        }
        LfgPacket::Reward(r) => script.lfg_completion_reward(reward(r)),
        LfgPacket::TeleportDenied(reason) => {
            debug!("ui_lfg: teleport denied, reason {reason}");
            script.lfg_teleport_denied(reason)
        }
        LfgPacket::OfferContinue(slot) => {
            script.lfg_offer_continue(slot);
            Vec::new()
        }
        LfgPacket::OpenFromGossip(slot) => {
            script.lfg_open_from_gossip(slot);
            Vec::new()
        }
    }
}

fn update(u: LfgUpdate) -> LfgUpdateView {
    LfgUpdateView {
        update_type: u.update_type,
        joined: u.joined,
        lfg_joined: u.lfg_joined,
        queued: u.queued,
        no_partial_clear: u.no_partial_clear,
        achievements: u.achievements,
        needs: u.needs,
        slots: u.slots,
        comment: u.comment,
    }
}

fn items(items: &[LfgRewardItem]) -> Vec<LfgRewardItemView> {
    items
        .iter()
        .map(|i| LfgRewardItemView {
            item: i.item,
            display: i.display,
            count: i.count,
        })
        .collect()
}

fn locks(members: Vec<LfgMemberLocks>) -> Vec<LfgMemberLocksView> {
    members
        .into_iter()
        .map(|m| LfgMemberLocksView {
            guid: m.guid,
            locks: m.locks,
        })
        .collect()
}

fn reward(r: LfgCompletionReward) -> LfgCompletionRewardView {
    LfgCompletionRewardView {
        random_slot: r.random_slot,
        actual_slot: r.actual_slot,
        first: r.first,
        strangers: r.strangers,
        base_money: r.base_money,
        base_xp: r.base_xp,
        variable_money: r.variable_money,
        variable_xp: r.variable_xp,
        items: items(&r.items),
    }
}

/// The engine's lines through the error display's queue, its kits through the UI sound path.
fn show(
    script: &UiScript,
    lines: Vec<LfgLine>,
    errors: &mut UiErrorKeys,
    sounds: &mut MessageSounds,
) {
    for line in lines {
        match line {
            LfgLine::Key(key) => errors.0.push(UiError::key(key)),
            LfgLine::Str(key, s) => errors.0.push(UiError::s(key, s)),
            LfgLine::Int(key, n) => errors.0.push(UiError::args(key, vec![FillArg::D(n)])),
            LfgLine::Money(key, copper) => {
                let get =
                    |k: &str| benilla_ui::strings::global(script.lua(), k).unwrap_or_default();
                let text = crate::ui_quest::coin_text(copper.max(0) as u32, &get);
                errors.0.push(UiError::s(key, text));
            }
            LfgLine::Sound(kit) => sounds.push_kit(kit),
        }
    }
}
