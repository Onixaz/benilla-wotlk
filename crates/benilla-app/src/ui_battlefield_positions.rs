//! Battleground teammate and flag positions on the world map: `MSG_BATTLEGROUND_PLAYER_POSITIONS`
//! kept as its handler writes it (`0x54b3f0`), read by the position verbs through the live view
//! when called (`0x54a040`, `0x54c2e0`, `0x54dcc0`); a name the getter asks fires
//! `WORLD_MAP_NAME_UPDATE` from its answer, once per ask (`0x5496c0`).

use std::sync::Mutex;
use std::time::{Duration, Instant};

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_assets::coords::bevy_to_wow;
use benilla_assets::LockRecover;
use benilla_protocol::messages::{BattlefieldPosition, BattlefieldPositions as PositionsPacket};
use benilla_ui::script::{BattlefieldFlagLive, BattlefieldPositionLive, WorldMapPoint};

use crate::names::NameCache;
use crate::net::{ClientCommand, NetCommands, ObjectStore};
use crate::ui_battlefield::BattlefieldQueue;
use crate::ui_party::GroupState;

/// `RequestBattlefieldPositions` (`0x54cf60`) sends at most once per 5000 ms.
pub(crate) const REQUEST_THROTTLE: Duration = Duration::from_millis(5000);

/// Eye of the Storm's `Map.dbc` row, whose one flag reverses the token (`0x54d0b5`).
const EYE_OF_THE_STORM: u32 = 566;

/// The positions the handler wrote, locked so the verbs can clear, stamp and file name asks while lent.
#[derive(Resource, Default)]
pub(crate) struct BattlefieldPositions(Mutex<Board>);

#[derive(Default)]
struct Board {
    /// The players in wire order (`0xbea180`, count `0xbea5b0`).
    players: Vec<BattlefieldPosition>,
    /// The two carriers by slot, guid 0 for none.
    carriers: [BattlefieldPosition; 2],
    /// The request stamp (`0xbea5b4`): no send before it.
    next_request: Option<Instant>,
    /// One guid per name ask the getter filed, each answered by its own event.
    name_asks: Vec<u64>,
}

impl BattlefieldPositions {
    fn board(&mut self) -> &mut Board {
        self.0.get_mut().unwrap_or_else(|p| p.into_inner())
    }

    /// `MSG_BATTLEGROUND_PLAYER_POSITIONS` (`0x54b3f0`): players as sent, carriers by slot.
    pub(crate) fn apply(&mut self, packet: PositionsPacket) {
        let board = self.board();
        board.players = packet.players;
        board.carriers = Default::default();
        for (slot, c) in board.carriers.iter_mut().zip(packet.carriers) {
            *slot = c;
        }
    }

    /// The status handler's queue-0 arm on the active slot zeroes the list (`0x54b165`).
    pub(crate) fn clear_players(&mut self) {
        self.board().players.clear();
    }

    /// The world entry's part (`0x54cb1b`): the list, the carriers and the request stamp.
    pub(crate) fn enter_world(&mut self) {
        let board = self.board();
        board.players.clear();
        board.carriers = Default::default();
        board.next_request = None;
    }

    /// The asks a name answer settles, each one `WORLD_MAP_NAME_UPDATE`.
    fn take_asks(&mut self, guid: u64) -> usize {
        let asks = &mut self.board().name_asks;
        let before = asks.len();
        asks.retain(|&g| g != guid);
        before - asks.len()
    }

    /// The request (`0x54cf60`): no active slot clears the list (`0x54cf7d`), else a send at most every 5000 ms.
    fn request(&self, active: bool, now: Instant) -> bool {
        let mut board = self.0.lock_recover();
        if !active {
            board.players.clear();
            return false;
        }
        if board.next_request.is_some_and(|t| now < t) {
            return false;
        }
        board.next_request = Some(now + REQUEST_THROTTLE);
        true
    }
}

/// The flag token (`0x54d010`): the carrier's side indexes `{AllianceFlag, HordeFlag}`, reversed in Eye of the Storm.
fn flag_token(side: i8, map: u32) -> Option<&'static str> {
    const TOKENS: [&str; 2] = ["AllianceFlag", "HordeFlag"];
    let side = usize::try_from(side).ok().filter(|&s| s < 2)?;
    Some(if map == EYE_OF_THE_STORM {
        TOKENS[usize::from(side == 0)]
    } else {
        TOKENS[side]
    })
}

/// What the position verbs read when called.
#[derive(SystemParam)]
pub(crate) struct PositionsView<'w, 's> {
    positions: Option<Res<'w, BattlefieldPositions>>,
    queue: Option<Res<'w, BattlefieldQueue>>,
    group: Option<Res<'w, GroupState>>,
    names: Option<Res<'w, NameCache>>,
    net: Option<Res<'w, NetCommands>>,
    maps: Option<Res<'w, benilla_assets::MapCatalogRes>>,
    areas: Option<Res<'w, crate::area::AreaTableRes>>,
    map: Option<Res<'w, benilla_world::world_map::CurrentMap>>,
    places: Query<'w, 's, &'static Transform, With<ObjectStore>>,
}

impl PositionsView<'_, '_> {
    fn board(&self) -> Option<std::sync::MutexGuard<'_, Board>> {
        Some(self.positions.as_deref()?.0.lock_recover())
    }

    /// The active map id (`0xbea564`), kept past the active slot's drop.
    fn active_map(&self) -> u32 {
        self.queue
            .as_deref()
            .map_or(0, BattlefieldQueue::active_map_id)
    }

    /// Whether `guid` is ours or our party's (`0x52c680`) or in our raid (`0x5726f0`).
    fn grouped(&self, me: Option<u64>, guid: u64) -> bool {
        guid != 0
            && (me == Some(guid)
                || self
                    .group
                    .as_deref()
                    .is_some_and(|g| g.in_party_or_raid(me, guid)))
    }

    /// The list's entries outside our party and raid, in wire order.
    fn visible(&self, me: Option<u64>) -> Vec<BattlefieldPosition> {
        let mut players = self.board().map(|b| b.players.clone()).unwrap_or_default();
        players.retain(|p| !self.grouped(me, p.guid));
        players
    }

    pub(crate) fn count(&self, me: Option<u64>) -> usize {
        self.visible(me).len()
    }

    /// `0x54c2e0`: a held player's position (`0x54c3eb`), else the packet's (`0x54c41c`); a nameless row asks (`0x54c489`).
    pub(crate) fn position(
        &self,
        me: Option<u64>,
        index: usize,
        player: impl Fn(u64) -> Option<Entity>,
    ) -> Option<BattlefieldPositionLive> {
        let p = *self.visible(me).get(index)?;
        let map = self.active_map();
        let at = match player(p.guid).and_then(|e| self.places.get(e).ok()) {
            Some(tf) => {
                let [x, y, z] = bevy_to_wow(tf.translation);
                WorldMapPoint { map, x, y, z }
            }
            None => WorldMapPoint::flat(map, p.x, p.y),
        };
        let name = self
            .names
            .as_deref()
            .zip(self.net.as_deref())
            .and_then(|(names, net)| names.full_name_record(p.guid, net));
        if name.is_none() {
            if let Some(mut board) = self.board() {
                board.name_asks.push(p.guid);
            }
        }
        Some(BattlefieldPositionLive { at, name })
    }

    /// The carrier count: 2 with the second slot held, else 1 with the first (`0x54a0e0`).
    pub(crate) fn flag_count(&self) -> usize {
        let Some(board) = self.board() else {
            return 0;
        };
        if board.carriers[1].guid != 0 {
            2
        } else {
            usize::from(board.carriers[0].guid != 0)
        }
    }

    /// `0x54dcc0`: the carrier's own or party position (`0x54dd39`), else the packet's (`0x54dd81`), and its token.
    pub(crate) fn flag(
        &self,
        me: Option<u64>,
        index: usize,
        player: impl Fn(u64) -> Option<Entity>,
        side: impl Fn(u64) -> Option<i8>,
    ) -> Option<BattlefieldFlagLive> {
        if index >= self.flag_count() {
            return None;
        }
        let c = self.board()?.carriers[index];
        let map = self.active_map();
        let here = self.map.as_deref().map_or(0, |m| m.0);
        let carrier = match player(c.guid) {
            Some(e) => self.places.get(e).ok().map(|tf| {
                let [x, y, z] = bevy_to_wow(tf.translation);
                WorldMapPoint { map: here, x, y, z }
            }),
            None => self.member_point(c.guid),
        };
        // The carrier's side, else ours flipped unless it is in our raid, else the slot.
        let faction = match side(c.guid) {
            Some(s) => s,
            None => match me.and_then(&side) {
                Some(mine) => {
                    let raid = self
                        .group
                        .as_deref()
                        .is_some_and(|g| g.in_raid_roster(me, c.guid));
                    if raid {
                        mine
                    } else {
                        i8::from(mine == 0)
                    }
                }
                None => index as i8,
            },
        };
        Some(BattlefieldFlagLive {
            carrier,
            packet: WorldMapPoint::flat(map, c.x, c.y),
            token: flag_token(faction, map),
        })
    }

    /// A party record's position on its zone's map (`0x544569`-`0x5445cd`), none without one.
    fn member_point(&self, guid: u64) -> Option<WorldMapPoint> {
        let stats = self.group.as_deref()?.stats.get(&guid)?;
        let area = self.areas.as_deref()?.0.get(u32::from(stats.zone?))?;
        let (x, y) = stats.position.unwrap_or_default();
        Some(WorldMapPoint::flat(area.map_id, f32::from(x), f32::from(y)))
    }

    /// `GetBattlefieldMapIconScale` (`0x54c740`): the active map's row, 1 without one.
    pub(crate) fn icon_scale(&self) -> f32 {
        self.maps
            .as_deref()
            .map_or(1.0, |m| m.0.icon_scale(self.active_map()))
    }

    /// `RequestBattlefieldPositions` (`0x54cf60`), sent from the verb.
    pub(crate) fn request(&self) {
        let (Some(positions), Some(net)) = (self.positions.as_deref(), self.net.as_deref()) else {
            return;
        };
        let active = self
            .queue
            .as_deref()
            .is_some_and(|q| q.active_bg_type() != 0);
        if positions.request(active, Instant::now()) {
            let _ = net.0.send(ClientCommand::RequestBattlefieldPositions);
        }
    }
}

/// The battlefield map's packet handlers.
mod net {
    use benilla_protocol::{SessionEvent, SessionEventKind};
    use bevy::prelude::*;

    use super::BattlefieldPositions;
    use crate::net::NetHandlerApp;
    use crate::ui_script::live::UiEvents;

    pub(super) fn register(app: &mut App) {
        use SessionEventKind as K;
        app.net_handler(K::BattlefieldPositions, on_positions)
            .net_handler(K::PlayerName, on_name_answer)
            .net_handler(K::PlayerNameMiss, on_name_answer)
            .net_handler(K::Disconnected, on_session_end);
    }

    fn on_positions(In(ev): In<SessionEvent>, mut positions: ResMut<BattlefieldPositions>) {
        if let SessionEvent::BattlefieldPositions(packet) = ev {
            positions.apply(packet);
        }
    }

    /// A name answer runs each world-map callback filed for it, named or denied (`0x67a2d0`).
    fn on_name_answer(
        In(ev): In<SessionEvent>,
        mut positions: ResMut<BattlefieldPositions>,
        mut ui: ResMut<UiEvents>,
    ) {
        use benilla_protocol::messages::NameQueryMiss;
        let guid = match ev {
            SessionEvent::PlayerName { guid, .. } => guid,
            SessionEvent::PlayerNameMiss {
                miss: NameQueryMiss::Retry,
                ..
            } => return,
            SessionEvent::PlayerNameMiss { guid, .. } => guid,
            _ => return,
        };
        for _ in 0..positions.take_asks(guid) {
            ui.fire("WORLD_MAP_NAME_UPDATE", Vec::new());
        }
    }

    /// The list and its stamp end with the session.
    fn on_session_end(In(_): In<SessionEvent>, mut positions: ResMut<BattlefieldPositions>) {
        *positions = BattlefieldPositions::default();
    }
}

pub(crate) struct BattlefieldPositionsPlugin;

impl Plugin for BattlefieldPositionsPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<BattlefieldPositions>();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(guid: u64, x: f32) -> BattlefieldPosition {
        BattlefieldPosition { guid, x, y: 0.0 }
    }

    #[test]
    fn the_request_throttle_is_the_references_five_seconds() {
        let s = BattlefieldPositions::default();
        let t0 = Instant::now();
        assert!(s.request(true, t0), "the first request goes out");
        assert!(!s.request(true, t0 + Duration::from_millis(4999)));
        assert!(s.request(true, t0 + REQUEST_THROTTLE));
        assert!(!s.request(true, t0 + REQUEST_THROTTLE + Duration::from_millis(1)));
    }

    /// Without an active slot the request clears the list and sends nothing (`0x54cf7d`).
    #[test]
    fn a_request_without_an_active_slot_clears_the_list() {
        let mut s = BattlefieldPositions::default();
        s.apply(PositionsPacket {
            players: vec![at(1, 1.0), at(2, 2.0)],
            carriers: vec![at(3, 3.0)],
        });
        assert!(!s.request(false, Instant::now()));
        let board = s.board();
        assert!(board.players.is_empty());
        assert_eq!(board.carriers[0].guid, 3, "the carriers stay");
    }

    /// A packet with one carrier zeroes the second slot (`0x54b3f0`).
    #[test]
    fn a_packet_rewrites_both_carrier_slots() {
        let mut s = BattlefieldPositions::default();
        s.apply(PositionsPacket {
            players: Vec::new(),
            carriers: vec![at(3, 3.0), at(4, 4.0)],
        });
        s.apply(PositionsPacket {
            players: Vec::new(),
            carriers: vec![at(5, 5.0)],
        });
        let board = s.board();
        assert_eq!((board.carriers[0].guid, board.carriers[1].guid), (5, 0));
    }

    /// Each ask the getter filed answers with its own event; another guid's answer settles none.
    #[test]
    fn each_name_ask_takes_its_own_answer() {
        let mut s = BattlefieldPositions::default();
        s.board().name_asks = vec![7, 8, 7];
        assert_eq!(s.take_asks(9), 0);
        assert_eq!(s.take_asks(7), 2);
        assert_eq!(s.take_asks(7), 0);
        assert_eq!(s.take_asks(8), 1);
    }

    #[test]
    fn the_flag_token_is_the_carriers_side_reversed_in_the_eye() {
        assert_eq!(flag_token(0, 489), Some("AllianceFlag"), "a Horde carrier");
        assert_eq!(flag_token(1, 489), Some("HordeFlag"));
        assert_eq!(flag_token(0, EYE_OF_THE_STORM), Some("HordeFlag"));
        assert_eq!(flag_token(-1, 489), None);
    }
}
