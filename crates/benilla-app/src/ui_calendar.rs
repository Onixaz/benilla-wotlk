//! The calendar feed: the holiday and map tables pushed once per VM, every calendar message handed
//! to the VM's `CGCalendar` port in wire order, the player, social and name facts it reads, and the
//! sends its verbs queue; the clock rides [`crate::minimap`]'s minute feed. A message waits in [`CalendarFeed`] until a VM is up, so one
//! landing between the login and the UI's first frame is applied in order.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use bevy::prelude::*;

use benilla_assets::{AssetSet, LockRecover, WorldAssets};
use benilla_protocol::messages::{
    CalendarCommand, CalendarEventFields, CalendarNewInvite, CalendarPacket,
};
use benilla_protocol::{SessionEvent, SessionEventKind};
use benilla_ui::script::{
    CalendarCatalogView, CalendarCommandResultView, CalendarEventRowView, CalendarEventUpdateView,
    CalendarEventView, CalendarHolidayRowView, CalendarHolidayView, CalendarInviteAddedView,
    CalendarInviteAlertView, CalendarInviteRefView, CalendarInviteeView, CalendarLockoutView,
    CalendarName, CalendarPlayerView, CalendarRequest, CalendarResetView, CalendarSnapshotView,
    CalendarStatusView, CalendarWire, UiScript,
};

use crate::char_select::ClientState;
use crate::chr_classes::ChrClassTable;
use crate::names::NameCache;
use crate::net::{
    ClientCommand, NetCommands, NetHandlerApp, ObjectStore, SelfGuid, SelfPlayer,
    WorldEnterCascadeMessage,
};
use crate::ui_party::GroupState;
use crate::ui_script::{UiFeed, VmMemo};

/// The holiday and map tables, absent when the client data failed to load.
#[derive(Resource, Clone)]
pub(crate) struct CalendarCatalog {
    pub(crate) view: Arc<CalendarCatalogView>,
    /// The client's Region (`[WoW Config] Region`).
    pub(crate) region: i32,
}

/// What waits for the VM, in wire order.
enum Pending {
    Packet(CalendarWire),
    EnterWorld,
    InvalidatePlayer(u64),
}

/// The feed's session state.
#[derive(Resource, Default)]
pub(crate) struct CalendarFeed {
    pending: Vec<Pending>,
}

pub(crate) struct UiCalendarPlugin;

impl Plugin for UiCalendarPlugin {
    fn build(&self, app: &mut App) {
        use SessionEventKind as K;
        app.init_resource::<CalendarFeed>()
            .add_systems(Startup, load_calendar_catalog.after(AssetSet::Open))
            .add_systems(
                Update,
                (
                    calendar_enter_world.in_set(UiFeed),
                    // Gated, so a frameless boot VM never takes a message the in-game VM needs;
                    // ahead of the world-entry fire, whose stock clock button reads the count.
                    // After the social feed, whose ignore list filters the days, and the mail
                    // systems that share its arrival state.
                    feed_calendar
                        .in_set(UiFeed)
                        .after(calendar_enter_world)
                        .after(crate::ui_social::feed_social)
                        .after(crate::ui_mail::send_query_next_mail_time_on_enter)
                        .after(
                            crate::ui_session::close_npc_session_out_of_range::<
                                crate::ui_mail::MailOpen,
                            >,
                        )
                        .before(crate::ui_unit::EnterWorldFeed)
                        .run_if(crate::ui_script::ingame_ui_up),
                ),
            )
            .add_systems(OnExit(ClientState::InWorld), end_session)
            .net_handler(K::Calendar, on_calendar)
            .net_handler(K::InvalidatePlayer, on_invalidate_player);
    }
}

fn load_calendar_catalog(mut commands: Commands, assets: Option<Res<WorldAssets>>) {
    let Some(assets) = assets else { return };
    let mut chain = assets.chain.lock_recover();
    match read_catalog(&mut chain) {
        Ok(view) => {
            info!("ui_calendar: {} holiday(s)", view.holidays.len());
            commands.insert_resource(CalendarCatalog {
                view: Arc::new(view),
                region: benilla_formats::client_region(&chain) as i32,
            });
        }
        Err(e) => warn!("ui_calendar: the holiday tables failed to load: {e:#}"),
    }
}

/// `Holidays.dbc` with its names and descriptions, and `Map.dbc`'s names.
pub(crate) fn read_catalog(
    chain: &mut benilla_formats::Chain,
) -> anyhow::Result<CalendarCatalogView> {
    let holidays = benilla_formats::load_holidays(chain)?;
    let maps = benilla_formats::load_map_catalog(chain)?;
    Ok(CalendarCatalogView {
        holidays: holidays
            .into_iter()
            .map(|h| CalendarHolidayView {
                id: h.id as u32,
                durations: h.durations,
                dates: h.dates,
                region: h.region,
                looping: h.looping,
                calendar_flags: h.calendar_flags,
                name: h.name,
                description: h.description,
                texture: h.texture,
                priority: h.priority,
                filter_type: h.filter_type,
                flags: h.flags,
            })
            .collect(),
        map_names: maps
            .names()
            .map(|(id, name)| (id, name.to_string()))
            .collect(),
    })
}

fn on_calendar(In(ev): In<SessionEvent>, mut feed: ResMut<CalendarFeed>) {
    if let SessionEvent::Calendar(packet) = ev {
        debug!("ui_calendar: {}", packet.name());
        feed.pending.push(Pending::Packet(wire(packet)));
    }
}

fn on_invalidate_player(In(ev): In<SessionEvent>, mut feed: ResMut<CalendarFeed>) {
    if let SessionEvent::InvalidatePlayer { guid } = ev {
        feed.pending.push(Pending::InvalidatePlayer(guid));
    }
}

/// The world-enter cascade's calendar leg (`CGCalendar::EnterWorld` `0x5be2a0`).
fn calendar_enter_world(
    mut cascades: MessageReader<WorldEnterCascadeMessage>,
    mut feed: ResMut<CalendarFeed>,
) {
    if cascades.read().next().is_some() {
        feed.pending.push(Pending::EnterWorld);
    }
}

fn end_session(mut feed: ResMut<CalendarFeed>) {
    *feed = CalendarFeed::default();
}

/// What the feed last pushed into this VM.
#[derive(Default)]
struct FeedMemory {
    names_generation: u64,
    /// The player's `PLAYER_GUILDID` last seen, `None` before the first.
    guild_id: Option<u32>,
}

/// A side by its faction group mask: 0 Horde, 1 Alliance, -1 neither (`0x6d6e90`).
fn side(mask: u32) -> i32 {
    if mask & 0x4 != 0 {
        0
    } else if mask & 0x2 != 0 {
        1
    } else {
        -1
    }
}

/// The player's facts the calendar reads.
fn player_view(
    store: &ObjectStore,
    guid: u64,
    factions: Option<&crate::target::ring::Factions>,
) -> CalendarPlayerView {
    let fields = &store.0;
    let catalog = factions.map(|f| f.catalog());
    let race_side = catalog
        .zip(fields.unit_race())
        .and_then(|(c, r)| c.race_template(r))
        .map_or(-1, |t| side(t.group_mask));
    let template_side = catalog
        .zip(fields.unit_faction_template())
        .and_then(|(c, t)| c.template(t))
        .map_or(-1, |t| side(t.group_mask));
    CalendarPlayerView {
        guid,
        level: fields.unit_level().unwrap_or(0) as u8,
        guild_id: fields.player_guild_id(),
        arena_teams: [0, 1, 2].map(|slot| fields.player_arena_team_info(slot, 0).unwrap_or(0)),
        pvp_faction: race_side,
        faction_group: template_side,
    }
}

#[allow(clippy::too_many_arguments)]
fn feed_calendar(
    script: Option<NonSendMut<UiScript>>,
    catalog: Option<Res<CalendarCatalog>>,
    mut feed: ResMut<CalendarFeed>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    self_guid: Res<SelfGuid>,
    factions: Option<Res<crate::target::ring::Factions>>,
    social: Res<crate::ui_social::SocialState>,
    group: Res<GroupState>,
    names: Res<NameCache>,
    classes: Option<Res<ChrClassTable>>,
    commands: Res<NetCommands>,
    mut mail: crate::ui_mail::MailArrival,
    mut memory: Local<VmMemo<FeedMemory>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let (memory, vm_reset) = memory.get_reset(&script);
    if vm_reset {
        if let Some(c) = catalog.as_ref() {
            script.set_calendar_catalog((*c.view).clone());
            script.set_calendar_region(c.region);
        }
        memory.names_generation = u64::MAX;
    }

    let player = self_q
        .single()
        .ok()
        .zip(self_guid.0)
        .map(|(store, guid)| player_view(store, guid, factions.as_deref()));
    let guild_id = player.as_ref().map(|p| p.guild_id);
    script.set_calendar_player(player);
    script.set_calendar_social(social.ignored_set(), social.friend_set());
    let raid: HashSet<u64> =
        if group.in_group && group.group_type == crate::ui_party::GROUPTYPE_RAID {
            group.members.iter().map(|m| m.guid).collect()
        } else {
            HashSet::new()
        };
    script.set_calendar_raid_members(raid);

    // `CGPlayer_C::OnGuildChanged` (`0x6df7a0`): the player's own guild field moved.
    if let Some(id) = guild_id {
        if memory.guild_id.is_some_and(|was| was != id) {
            script.calendar_guild_changed(id != 0);
        }
        memory.guild_id = Some(id);
    }

    // Names first, so a packet naming a cached player finds it cached, as the reference's does.
    let source = NameSource {
        names: &names,
        classes: classes.as_deref(),
        commands: &commands,
        own: self_q
            .single()
            .ok()
            .and_then(|store| store.0.unit_class())
            .zip(self_guid.0)
            .map(|(class, guid)| (guid, class)),
    };
    push_names(&mut script, &source, memory, false);
    for item in std::mem::take(&mut feed.pending) {
        match item {
            Pending::EnterWorld => script.calendar_enter_world(),
            Pending::InvalidatePlayer(guid) => script.calendar_invalidate_player(guid),
            Pending::Packet(wire) => {
                prefetch_names(&mut script, &wire, &source);
                script.calendar_packet(wire);
            }
        }
    }

    for request in script.take_calendar_requests() {
        debug!("ui_calendar: send {request:?}");
        match command(request) {
            Some(c) => {
                let _ = commands.0.send(ClientCommand::Calendar(c));
            }
            None => mail.arrive(0.0, "calendar"),
        }
    }
    push_names(&mut script, &source, memory, true);
}

/// Where the calendar's names come from: the name cache, with the player's own class from its
/// own fields, which no name query answers.
struct NameSource<'a> {
    names: &'a NameCache,
    classes: Option<&'a ChrClassTable>,
    commands: &'a NetCommands,
    own: Option<(u64, u8)>,
}

impl NameSource<'_> {
    /// One name as the calendar reads it, `None` while in flight; asking queues the query.
    fn name_of(&self, guid: u64) -> Option<CalendarName> {
        let name = self.names.resolve(guid, self.commands)?.to_string();
        let class = self
            .names
            .player_traits(guid)
            .map(|(_, class, _)| class)
            .or_else(|| self.own.filter(|&(g, _)| g == guid).map(|(_, c)| c))
            .map(u32::from);
        let class_name = class
            .and_then(|c| self.classes?.0.name(c))
            .unwrap_or_default()
            .to_string();
        let class_file = class
            .and_then(|c| self.classes?.0.file(c))
            .unwrap_or_default()
            .to_string();
        Some(CalendarName {
            name,
            class_name,
            class_file,
        })
    }
}

/// The names the calendar wants, pushed when the cache moved or `force`d after a packet.
fn push_names(script: &mut UiScript, source: &NameSource, memory: &mut FeedMemory, force: bool) {
    if !force && source.names.generation() == memory.names_generation {
        return;
    }
    memory.names_generation = source.names.generation();
    let known: HashMap<u64, CalendarName> = script
        .calendar_names_wanted()
        .into_iter()
        .filter_map(|g| Some((g, source.name_of(g)?)))
        .collect();
    script.set_calendar_names(known);
}

/// The cached names of the players a message names, pushed before it applies.
fn prefetch_names(script: &mut UiScript, wire: &CalendarWire, source: &NameSource) {
    let guids: Vec<u64> = match wire {
        CalendarWire::SendEvent(e) => e
            .invites
            .iter()
            .map(|i| i.guid)
            .chain([e.creator])
            .collect(),
        CalendarWire::InitialInvites(list) => list.iter().map(|&(g, _)| g).collect(),
        CalendarWire::Invite(i) => vec![i.guid],
        _ => return,
    };
    let known: HashMap<u64, CalendarName> = guids
        .into_iter()
        .filter(|&g| g != 0)
        .filter_map(|g| Some((g, source.name_of(g)?)))
        .collect();
    if !known.is_empty() {
        script.set_calendar_names(known);
    }
}

/// A request as its send; `None` for the mailbox re-query.
fn command(request: CalendarRequest) -> Option<CalendarCommand> {
    use CalendarCommand as C;
    use CalendarRequest as R;
    Some(match request {
        R::GetCalendar => C::GetCalendar,
        R::GetNumPending => C::GetNumPending,
        R::GetEvent { event_id } => C::GetEvent { event_id },
        R::GuildFilter {
            min_level,
            max_level,
            min_rank,
        } => C::GuildFilter {
            min_level,
            max_level,
            min_rank,
        },
        R::ArenaTeam { team_id } => C::ArenaTeam { team_id },
        R::AddEvent {
            title,
            description,
            event_type,
            repeat,
            max_size,
            texture,
            date,
            lockout_date,
            flags,
            invites,
        } => C::AddEvent {
            event: CalendarEventFields {
                title,
                description,
                event_type,
                repeat,
                max_size,
                texture,
                date,
                lockout_date,
                flags,
            },
            invites: invites.map(|list| {
                list.into_iter()
                    .map(|i| CalendarNewInvite {
                        guid: i.guid,
                        status: i.status,
                        moderator: i.moderator,
                    })
                    .collect()
            }),
        },
        R::UpdateEvent {
            event_id,
            moderator_invite,
            title,
            description,
            event_type,
            repeat,
            max_size,
            texture,
            date,
            lockout_date,
            flags,
        } => C::UpdateEvent {
            event_id,
            moderator_invite,
            event: CalendarEventFields {
                title,
                description,
                event_type,
                repeat,
                max_size,
                texture,
                date,
                lockout_date,
                flags,
            },
        },
        R::RemoveEvent {
            event_id,
            moderator_invite,
            guild,
        } => C::RemoveEvent {
            event_id,
            moderator_invite,
            guild,
        },
        R::RemoveDayEvent {
            event_id,
            invite_id,
            flags,
        } => C::RemoveDayEvent {
            event_id,
            invite_id,
            flags,
        },
        R::CopyEvent {
            event_id,
            invite_id,
            date,
        } => C::CopyEvent {
            event_id,
            invite_id,
            date,
        },
        R::Invite {
            event_id,
            moderator_invite,
            name,
            pre_invite,
            guild_event,
        } => C::Invite {
            event_id,
            moderator_invite,
            name,
            pre_invite,
            guild_event,
        },
        R::Rsvp {
            event_id,
            invite_id,
            status,
        } => C::Rsvp {
            event_id,
            invite_id,
            status,
        },
        R::RemoveInvite {
            guid,
            invite_id,
            moderator_invite,
            event_id,
        } => C::RemoveInvite {
            guid,
            invite_id,
            moderator_invite,
            event_id,
        },
        R::SetStatus {
            guid,
            event_id,
            invite_id,
            moderator_invite,
            status,
        } => C::SetStatus {
            guid,
            event_id,
            invite_id,
            moderator_invite,
            status,
        },
        R::SetModerator {
            guid,
            event_id,
            invite_id,
            moderator_invite,
            status,
        } => C::SetModerator {
            guid,
            event_id,
            invite_id,
            moderator_invite,
            status,
        },
        R::Complain {
            creator,
            event_id,
            invite_id,
        } => C::Complain {
            creator,
            event_id,
            invite_id,
        },
        R::SignUp {
            event_id,
            tentative,
        } => C::SignUp {
            event_id,
            tentative,
        },
        R::ReceivedMail => return None,
    })
}

/// A message as the engine takes it.
fn wire(packet: CalendarPacket) -> CalendarWire {
    use CalendarPacket as P;
    use CalendarWire as W;
    let lockout = |l: benilla_protocol::messages::CalendarLockout| CalendarLockoutView {
        map: l.map,
        difficulty: l.difficulty,
        reset: l.reset,
        instance_id: l.instance_id,
    };
    match packet {
        P::SendCalendar(s) => W::SendCalendar(Box::new(CalendarSnapshotView {
            invites: s
                .invites
                .into_iter()
                .map(|i| CalendarInviteRefView {
                    event_id: i.event_id,
                    invite_id: i.invite_id,
                    status: i.status,
                    rank: i.rank,
                    invite_type: i.invite_type,
                    inviter: i.inviter,
                })
                .collect(),
            events: s
                .events
                .into_iter()
                .map(|e| CalendarEventRowView {
                    event_id: e.event_id,
                    title: e.title,
                    event_type: e.event_type,
                    start: e.start,
                    flags: e.flags,
                    texture: e.texture,
                    creator: e.creator,
                })
                .collect(),
            server_now: s.server_now,
            server_date: s.server_date,
            lockouts: s.lockouts.into_iter().map(lockout).collect(),
            reset_base: s.reset_base,
            resets: s
                .resets
                .into_iter()
                .map(|r| CalendarResetView {
                    map: r.map,
                    period: r.period,
                    offset: r.offset,
                })
                .collect(),
            holidays: s
                .holidays
                .into_iter()
                .map(|h| CalendarHolidayRowView {
                    id: h.id,
                    region: h.region,
                    looping: h.looping,
                    priority: h.priority,
                    filter_type: h.filter_type,
                    dates: h.dates,
                    durations: h.durations,
                    flags: h.flags,
                    texture: h.texture,
                })
                .collect(),
        })),
        P::SendEvent(e) => W::SendEvent(Box::new(CalendarEventView {
            send_type: e.send_type,
            creator: e.creator,
            event_id: e.event_id,
            title: e.title,
            description: e.description,
            event_type: e.event_type,
            repeat: e.repeat,
            max_size: e.max_size,
            texture: e.texture,
            flags: e.flags,
            date: e.date,
            lockout_date: e.lockout_date,
            guild_id: e.guild_id,
            invites: e
                .invites
                .into_iter()
                .map(|i| CalendarInviteeView {
                    guid: i.guid,
                    level: i.level,
                    status: i.status,
                    moderator: i.moderator,
                    invite_type: i.invite_type,
                    invite_id: i.invite_id,
                    response_time: i.response_time,
                    notes: i.notes,
                })
                .collect(),
        })),
        P::InitialInvites(list) => W::InitialInvites(list),
        P::Invite(i) => W::Invite(CalendarInviteAddedView {
            guid: i.guid,
            event_id: i.event_id,
            invite_id: i.invite_id,
            level: i.level,
            status: i.status,
            invite_type: i.invite_type,
            response_time: i.response_time,
            clear_pending: i.clear_pending,
        }),
        P::InviteRemoved {
            guid,
            event_id,
            flags,
            clear_pending,
        } => W::InviteRemoved {
            guid,
            event_id,
            flags,
            clear_pending,
        },
        P::Status(s) => W::Status(CalendarStatusView {
            guid: s.guid,
            event_id: s.event_id,
            date: s.date,
            flags: s.flags,
            status: s.status,
            clear_pending: s.clear_pending,
            response_time: s.response_time,
        }),
        P::CommandResult(r) => W::CommandResult(CalendarCommandResultView {
            command: r.command,
            event_name: r.event_name,
            name: r.name,
            result: r.result,
        }),
        P::LockoutAdded { date, lockout: l } => W::LockoutAdded {
            date,
            lockout: lockout(l),
        },
        P::LockoutRemoved { map, lockout: l } => W::LockoutRemoved {
            map,
            lockout: l.map(lockout),
        },
        P::LockoutUpdated {
            date,
            map,
            difficulty,
            old_reset,
            new_reset,
        } => W::LockoutUpdated {
            date,
            map,
            difficulty,
            old_reset,
            new_reset,
        },
        P::InviteAlert(a) => W::InviteAlert(CalendarInviteAlertView {
            event_id: a.event_id,
            title: a.title,
            start: a.start,
            flags: a.flags,
            event_type: a.event_type,
            texture: a.texture,
            invite_id: a.invite_id,
            status: a.status,
            rank: a.rank,
            creator: a.creator,
            inviter: a.inviter,
        }),
        P::InviteRemovedAlert {
            event_id,
            date,
            flags,
            status,
        } => W::InviteRemovedAlert {
            event_id,
            date,
            flags,
            status,
        },
        P::InviteStatusAlert {
            event_id,
            date,
            flags,
            status,
        } => W::InviteStatusAlert {
            event_id,
            date,
            flags,
            status,
        },
        P::EventRemovedAlert {
            clear_pending,
            event_id,
            date,
        } => W::EventRemovedAlert {
            clear_pending,
            event_id,
            date,
        },
        P::EventUpdatedAlert(u) => W::EventUpdatedAlert(Box::new(CalendarEventUpdateView {
            clear_pending: u.clear_pending,
            event_id: u.event_id,
            old_date: u.old_date,
            flags: u.flags,
            date: u.date,
            event_type: u.event_type,
            texture: u.texture,
            title: u.title,
            description: u.description,
            repeat: u.repeat,
            max_size: u.max_size,
            lockout_date: u.lockout_date,
        })),
        P::ModeratorStatusAlert {
            guid,
            event_id,
            status,
            clear_pending,
        } => W::ModeratorStatusAlert {
            guid,
            event_id,
            status,
            clear_pending,
        },
        P::NumPending(n) => W::NumPending(n),
        P::InviteNotes {
            guid,
            event_id,
            notes,
            clear_pending,
        } => W::InviteNotes {
            guid,
            event_id,
            notes,
            clear_pending,
        },
        P::InviteNotesAlert { event_id, notes } => W::InviteNotesAlert { event_id, notes },
        P::ClearPendingAction => W::ClearPendingAction,
    }
}
