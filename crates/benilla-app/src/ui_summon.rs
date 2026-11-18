//! Being summoned: `SMSG_SUMMON_REQUEST` latches the question and fires `CONFIRM_SUMMON` with no
//! arguments from its handler (`0x6d8680` -> `0x513c21`); `ConfirmSummon` and `CancelSummon`
//! answer it with `CMSG_SUMMON_RESPONSE`, the summoner's guid and an agree byte, and
//! `CANCEL_SUMMON` hides the dialog.
//!
//! - The latch writes four globals, the guid's two halves, the zone and an absolute deadline, then
//!   a bare event signal; the dialog's getters read them at call time through the live view.
//! - The request asks the active player first (`0x6d86c6`): with no player object nothing happens;
//!   dead or a ghost, it declines at once and shows nothing (`0x6d86fc`), so a live question is
//!   untouched. The test: health <= 0, or a player with the `PLAYER_FLAGS` ghost bit (a ghost's
//!   health is 1).
//! - `GetSummonConfirmSummoner()` answers `""` on a cache miss and queries the name; the popup
//!   re-reads it every frame (`StaticPopup.lua:1746`) until it lands.
//! - `ConfirmSummon()` checks no deadline, so two Accepts are two packets; the server checks its
//!   own expiry.
//! - `CANCEL_SUMMON` fires for `SMSG_SUMMON_CANCEL` and for the three ways out of a group: the kick,
//!   the disband and the leave ack ([`cancel`]).
//!
//! Deviation: the latch is dropped at session end and an answer with nothing latched sends nothing,
//! so no stale or zero guid reaches the server. The reference never clears the guid or zone, only
//! the deadline, so its answers send whatever is left.

use benilla_ui::script::UiScript;
use bevy::prelude::*;

use crate::area::AreaTableRes;
use crate::net::{ClientCommand, GuidIndex, NetCommands, ObjectStore, SelfGuid};
use crate::ui_script::live::UiEvents;
use crate::ui_script::UiDrain;

/// The latched question, the reference's four globals: written by [`net::request`], read by the
/// getters through [`SummonView`] and by [`drain_summon`].
#[derive(Resource, Default)]
pub(crate) struct SummonState {
    /// Echoed by the answers; 0 when nothing has asked this session.
    summoner: u64,
    /// The summoner's `AreaTable` id, not ours and not a map id.
    zone: u32,
    /// An absolute ms stamp there; here `Time<Real>` seconds, as the server's window runs in real
    /// time. `None` is the zeroed deadline.
    expires_at: Option<f64>,
    /// The name cache's record for the summoner, `""` until it lands.
    name: String,
    /// The zone's `AreaTable.dbc` row name, `""` with none.
    area: String,
}

impl SummonState {
    /// The latch.
    fn latch(&mut self, summoner: u64, zone: u32, delay_ms: u32, now: f64) {
        self.summoner = summoner;
        self.zone = zone;
        self.expires_at = Some(now + f64::from(delay_ms) / 1000.0);
    }

    /// A UI load zeroes the deadline alone (`0x52aa20`); the summoner and zone stay.
    pub(crate) fn reset_at_ui_load(&mut self) {
        self.expires_at = None;
    }

    fn pending(&self) -> Option<u64> {
        (self.summoner != 0).then_some(self.summoner)
    }

    /// Ms left, clamped at zero; the binding truncates to seconds. Both stamps are whole ms before the subtraction, as the deadline and the clock are (`0x5164c8`), so the frame that latched reads the packet's delay exactly.
    fn time_left_ms(&self, now: f64) -> u32 {
        let ms = |secs: f64| (secs * 1000.0).round() as i64;
        self.expires_at.map_or(0, |at| {
            (ms(at) - ms(now)).clamp(0, i64::from(u32::MAX)) as u32
        })
    }
}

/// `CANCEL_SUMMON` from the calling handler; the latch stays, as the reference's does.
pub(crate) fn cancel(ui: &mut UiEvents) {
    trace_fire("CANCEL_SUMMON");
    ui.fire("CANCEL_SUMMON", Vec::new());
}

/// The `summon` trace's middle link, between the packet and the response.
fn trace_fire(event: &str) {
    if benilla_assets::trace::enabled_for("summon") {
        benilla_assets::trace::line("summon", &format!("fire {event}"));
    }
}

/// What the three summon getters read at call time, part of the VM's live view: the latch, which
/// holds the cache's record and the zone's row as they are when read.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct SummonView<'w> {
    summon: Option<Res<'w, SummonState>>,
    time: Option<Res<'w, Time<Real>>>,
}

impl SummonView<'_> {
    /// The name cache's record for the latched guid (`0x516510`), `""` until it lands.
    pub(crate) fn summoner(&self) -> String {
        self.summon
            .as_ref()
            .map_or_else(String::new, |s| s.name.clone())
    }

    /// `GetSummonConfirmAreaName()` (`0x516580`).
    pub(crate) fn area(&self) -> String {
        self.summon
            .as_ref()
            .map_or_else(String::new, |s| s.area.clone())
    }

    /// `GetSummonConfirmTimeLeft()`'s milliseconds (`0x5164b0`).
    pub(crate) fn time_left_ms(&self) -> u32 {
        let now = self.time.as_ref().map_or(0.0, |t| t.elapsed_secs_f64());
        self.summon.as_ref().map_or(0, |s| s.time_left_ms(now))
    }
}

/// `GetSummonConfirmAreaName()`: the summoner's bare `AreaTable.dbc` row name or `""`, with no
/// parent walk and no GlobalString tail, unlike [`crate::ui_binder`]'s chain.
fn area_name(zone: u32, areas: Option<&AreaTableRes>) -> String {
    areas
        .and_then(|a| a.0.name(zone))
        .unwrap_or_default()
        .to_string()
}

/// Whether `ConfirmSummon` sends (`0x51b07e`-`0x51b0b0`): not while dead or a ghost, nor while the
/// unit flags hold stunned `0x40000`, in combat `0x80000`, confused `0x400000` or fleeing
/// `0x800000`. With no player object the reference faults; here it sends nothing.
fn may_accept(player: Option<&ObjectStore>) -> bool {
    const REFUSING: u32 = 0x0004_0000 | 0x0008_0000 | 0x0040_0000 | 0x0080_0000;
    player.is_some_and(|p| !p.0.is_dead_or_ghost() && p.0.unit_flags() & REFUSING == 0)
}

/// Turn each answer into `CMSG_SUMMON_RESPONSE`, gated on a latched summoner, never the deadline:
/// an Accept also on [`may_accept`], a Cancel on nothing more (`0x51b110`).
fn drain_summon(
    (script, _live): (Option<NonSendMut<UiScript>>, crate::ui_script::LendLive),
    summon: Res<SummonState>,
    commands: Res<NetCommands>,
    self_guid: Res<SelfGuid>,
    index: Res<GuidIndex>,
    stores: Query<&ObjectStore>,
) {
    let Some(mut script) = script else {
        return;
    };
    let answers = script.take_summon_answers();
    if answers.is_empty() {
        return;
    }
    let Some(summoner) = summon.pending() else {
        debug!("ui_summon: a summon answer with nothing latched — nothing to answer with");
        return;
    };
    let player = self_guid
        .0
        .and_then(|g| index.0.get(&g))
        .and_then(|e| stores.get(*e).ok());
    for agree in answers {
        if agree && !may_accept(player) {
            continue;
        }
        if benilla_assets::trace::enabled_for("summon") {
            benilla_assets::trace::line(
                "summon",
                &format!("SEND CMSG_SUMMON_RESPONSE summoner={summoner:#x} agree={agree}"),
            );
        }
        let _ = commands
            .0
            .send(ClientCommand::SummonResponse { summoner, agree });
    }
}

/// The summon's packet handlers.
pub(crate) mod net {
    use bevy::prelude::*;

    use super::SummonState;
    use benilla_protocol::{SessionEvent, SessionEventKind};

    use crate::net::NetHandlerApp;
    use crate::net::{ClientCommand, GuidIndex, NetCommands, ObjectStore, SelfGuid};
    use crate::ui_script::live::UiEvents;

    pub(super) fn register(app: &mut App) {
        app.net_handler(SessionEventKind::SummonRequest, on_request)
            .net_handler(SessionEventKind::SummonCancel, on_cancel)
            .net_handler(SessionEventKind::PlayerName, on_summoner_name)
            .net_handler(SessionEventKind::Disconnected, on_session_end);
    }

    /// The summoner's name landing in the cache: the getter's next read finds it.
    fn on_summoner_name(In(ev): In<SessionEvent>, mut summon: ResMut<SummonState>) {
        if let SessionEvent::PlayerName { guid, name, .. } = ev {
            if summon.pending() == Some(guid) {
                summon.name = name;
            }
        }
    }

    /// The latch dies with the session (the module's deviation); a `/reload` keeps the summoner
    /// and zone and zeroes the deadline (`0x52aa20`), so the time left reads 0.
    fn on_session_end(In(_): In<SessionEvent>, mut summon: ResMut<SummonState>) {
        *summon = SummonState::default();
    }

    /// The reference's dead test over the self object, not `unit_reads_dead`.
    fn on_request(
        In(ev): In<SessionEvent>,
        mut summon: ResMut<SummonState>,
        self_guid: Res<SelfGuid>,
        index: Res<GuidIndex>,
        stores: Query<&ObjectStore>,
        real_clock: Res<Time<Real>>,
        commands: Res<NetCommands>,
        mut ui: ResMut<UiEvents>,
        names: Res<crate::names::NameCache>,
        areas: Option<Res<crate::area::AreaTableRes>>,
    ) {
        if let SessionEvent::SummonRequest {
            summoner,
            zone,
            delay_ms,
        } = ev
        {
            let player = self_guid
                .0
                .and_then(|g| index.0.get(&g))
                .and_then(|e| stores.get(*e).ok())
                .map(|s| s.0.is_dead_or_ghost());
            let decline = request(
                summoner,
                zone,
                delay_ms,
                player,
                real_clock.elapsed_secs_f64(),
                &mut summon,
            );
            match decline {
                Some(true) => {
                    let _ = commands.0.send(ClientCommand::SummonResponse {
                        summoner,
                        agree: false,
                    });
                }
                // The latch's bare signal (`0x513c21`); a cache miss asks for the name.
                Some(false) => {
                    summon.name = match summoner {
                        0 => String::new(),
                        guid => names
                            .resolve(guid, &commands)
                            .unwrap_or_default()
                            .to_owned(),
                    };
                    summon.area = super::area_name(zone, areas.as_deref());
                    super::trace_fire("CONFIRM_SUMMON");
                    ui.fire("CONFIRM_SUMMON", Vec::new());
                }
                None => {}
            }
        }
    }

    /// `SMSG_SUMMON_CANCEL` (`0x6cbcf0`): `CANCEL_SUMMON`, nothing else.
    fn on_cancel(In(ev): In<SessionEvent>, mut ui: ResMut<UiEvents>) {
        if let SessionEvent::SummonCancel = ev {
            super::cancel(&mut ui);
        }
    }

    /// `SMSG_SUMMON_REQUEST` (`0x6d8680`): `dead_or_ghost` is `None` with no player object, which
    /// does nothing. A living player latches the offer and is asked; a dead one touches nothing,
    /// so a live question survives, and declines. `Some(true)` asks for the decline.
    pub(crate) fn request(
        summoner: u64,
        zone: u32,
        delay_ms: u32,
        dead_or_ghost: Option<bool>,
        now: f64,
        summon: &mut SummonState,
    ) -> Option<bool> {
        // The `summon` trace's first link, with the gate's verdict: a refusal looks like a bug.
        if benilla_assets::trace::enabled_for("summon") {
            benilla_assets::trace::line(
                "summon",
                &format!(
                    "recv SMSG_SUMMON_REQUEST summoner={summoner:#x} zone={zone} \
                     delay_ms={delay_ms} dead_or_ghost={dead_or_ghost:?}"
                ),
            );
        }
        match dead_or_ghost {
            None => {
                debug!("ui_summon: summon request from {summoner:#x} with no player object");
                None
            }
            Some(true) => {
                debug!("ui_summon: summon request from {summoner:#x} declined — dead or a ghost");
                Some(true)
            }
            Some(false) => {
                summon.latch(summoner, zone, delay_ms, now);
                Some(false)
            }
        }
    }
}

/// The summon flow: the question's handlers and its answers.
pub(crate) struct UiSummonPlugin;

impl Plugin for UiSummonPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<SummonState>()
            .add_systems(Update, drain_summon.in_set(UiDrain));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each request is its own dialog from its handler: the same summoner asking twice fires twice,
    /// and the guid outlives the fire.
    #[test]
    fn asking_twice_fires_two_dialogs() {
        let mut summon = SummonState::default();
        assert_eq!(summon.pending(), None);
        let ask = |summon: &mut SummonState, now| {
            net::request(0x2a, 1519, 120_000, Some(false), now, summon)
        };
        assert_eq!(ask(&mut summon, 100.0), Some(false), "a dialog");
        assert_eq!(summon.pending(), Some(0x2a), "the guid outlives the fire");
        assert_eq!(ask(&mut summon, 200.0), Some(false), "a second dialog");
    }

    /// The reference's countdown read, in its three cases: never armed, running, expired.
    #[test]
    fn the_countdown_runs_from_the_packets_delay_and_never_goes_negative() {
        let mut summon = SummonState::default();
        assert_eq!(summon.time_left_ms(0.0), 0, "a zeroed deadline reads 0");

        summon.latch(0x2a, 1519, 120_000, 100.0);
        assert_eq!(summon.time_left_ms(100.0), 120_000);
        assert_eq!(summon.time_left_ms(160.0), 60_000);
        assert_eq!(summon.time_left_ms(220.0), 0, "expired, not negative");
        assert_eq!(summon.time_left_ms(9_999.0), 0);
        // A clock whose seconds do not survive the add: the latching frame still reads the whole delay (`0x5164c8`).
        let now = 36.886_194_806_050_03;
        summon.latch(0x2a, 1519, 120_000, now);
        assert_eq!(summon.time_left_ms(now) / 1000, 120);
    }

    /// `0x6d86fc`: a dead or ghost player declines and leaves a live question exactly as it was;
    /// with no player object nothing happens at all (`0x6d86c9`).
    #[test]
    fn a_dead_player_declines_and_latches_nothing() {
        let mut summon = SummonState::default();
        assert_eq!(
            net::request(0x2a, 1519, 120_000, Some(true), 100.0, &mut summon),
            Some(true)
        );
        assert_eq!(summon.pending(), None);
        assert_eq!(summon.time_left_ms(100.0), 0);

        assert_eq!(
            net::request(0x2a, 1519, 120_000, None, 100.0, &mut summon),
            None
        );
        assert_eq!(summon.pending(), None, "no player object, no latch");

        // A live question, then a refused second request.
        assert_eq!(
            net::request(0x2a, 1519, 120_000, Some(false), 100.0, &mut summon),
            Some(false)
        );
        assert_eq!(
            net::request(0x99, 1, 120_000, Some(true), 150.0, &mut summon),
            Some(true)
        );
        assert_eq!(
            summon.pending(),
            Some(0x2a),
            "the live offer is undisturbed"
        );
        assert_eq!(summon.zone, 1519);
    }

    /// The cancel fires from its handler and keeps the latch.
    #[test]
    fn a_cancel_fires_and_keeps_the_latch() {
        let mut summon = SummonState::default();
        summon.latch(0x2a, 1519, 120_000, 100.0);
        let mut ui = UiEvents::default();
        cancel(&mut ui);
        assert_eq!(ui.names(), ["CANCEL_SUMMON"]);
        assert_eq!(summon.pending(), Some(0x2a));
    }

    /// A UI load zeroes the deadline alone (`0x52aa20`): the time left reads 0, the latch stays.
    #[test]
    fn a_ui_load_zeroes_the_deadline_and_keeps_the_latch() {
        let mut summon = SummonState::default();
        summon.latch(0x2a, 1519, 120_000, 100.0);
        summon.reset_at_ui_load();
        assert_eq!(summon.time_left_ms(100.0), 0);
        assert_eq!((summon.pending(), summon.zone), (Some(0x2a), 1519));
    }

    /// `0x51b07e`-`0x51b0b0`: alive with none of the four flags; no player object refuses.
    #[test]
    fn accepting_is_refused_dead_stunned_in_combat_confused_or_fleeing() {
        use benilla_protocol::field::{FIELD_UNIT_FLAGS, FIELD_UNIT_HEALTH, FIELD_UNIT_MAXHEALTH};
        let player = |hp: u32, flags: u32| {
            ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                (FIELD_UNIT_HEALTH, hp),
                (FIELD_UNIT_MAXHEALTH, 100),
                (FIELD_UNIT_FLAGS, flags),
            ]))
        };
        assert!(may_accept(Some(&player(100, 0))));
        assert!(may_accept(Some(&player(100, 0x0000_0008))), "another flag");
        assert!(!may_accept(Some(&player(0, 0))), "dead");
        for flag in [0x0004_0000, 0x0008_0000, 0x0040_0000, 0x0080_0000] {
            assert!(!may_accept(Some(&player(100, flag))), "{flag:#x}");
        }
        assert!(!may_accept(None));
    }

    /// No parent-zone hop and no GlobalString tail, unlike [`crate::ui_binder`]'s chain.
    #[test]
    fn the_area_name_is_the_bare_row_with_no_fallback_chain() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let areas = AreaTableRes(
            benilla_formats::load_area_table_catalog(&mut chain).expect("AreaTable.dbc"),
        );

        assert_eq!(area_name(1519, Some(&areas)), "Stormwind City");
        // 186 is Dolanaar, a sub-area of Teldrassil: the row itself answers.
        assert_eq!(area_name(186, Some(&areas)), "Dolanaar");
        assert_eq!(area_name(0xffff, Some(&areas)), "", "no row, no name");
        assert_eq!(area_name(1519, None), "", "no catalog, no name");
    }

    /// The module's deviation: an answer with nothing latched has nothing to send.
    #[test]
    fn nothing_pending_means_nothing_to_answer_with() {
        let summon = SummonState::default();
        assert_eq!(summon.pending(), None);
    }
}
