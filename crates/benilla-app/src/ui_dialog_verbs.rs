//! The app half of the verbs behind the stock `StaticPopup.lua` dialogs for the instance boot countdown and the area spirit healer's wave: their state, which the verbs read live, and the events their packet handlers fire (`0x513ad0`, `0x524b10`); the battleground queue is `crate::ui_battlefield`'s.

use std::time::Instant;

use bevy::ecs::system::NonSendMut;
use bevy::prelude::*;

use crate::net::{ClientCommand, NetCommands, ObjectStore, SelfPlayer};

/// The instance boot deadline, which `GetInstanceBootTimeRemaining` counts down at the call (`0x5162e0`).
#[derive(Resource, Default)]
pub(crate) struct InstanceBoot {
    deadline: Option<Instant>,
}

impl InstanceBoot {
    /// The boot setter (`0x513ad0`): a positive delay as `i32` arms the deadline, anything else clears it; the event it fires.
    pub(crate) fn set(&mut self, delay_ms: u32, now: Instant) -> &'static str {
        if delay_ms as i32 > 0 {
            self.deadline = Some(now + std::time::Duration::from_millis(u64::from(delay_ms)));
            "INSTANCE_BOOT_START"
        } else {
            self.deadline = None;
            "INSTANCE_BOOT_STOP"
        }
    }

    /// A UI load zeroes the deadline (`0x52aa08`), with no event.
    pub(crate) fn reset_at_ui_load(&mut self) {
        self.deadline = None;
    }

    /// Whole seconds left, 0 when idle or past, as the reference divides a clamped ms remainder.
    pub(crate) fn secs(&self, now: Instant) -> u32 {
        whole_secs(self.deadline, now)
    }
}

/// `max(deadline - now, 0) / 1000`, 0 with no deadline (`0x5162e0`, `0x516b90`).
fn whole_secs(deadline: Option<Instant>, now: Instant) -> u32 {
    deadline
        .map(|d| d.saturating_duration_since(now).as_secs())
        .map_or(0, |s| u32::try_from(s).unwrap_or(u32::MAX))
}

/// The zero-delay reason's line (`0x6e3be3`'s table); 1-4 name one.
fn raid_group_only_key(reason: u32) -> Option<&'static str> {
    Some(match reason {
        1 => "ERR_RAID_GROUP_LOWLEVEL",
        2 => "ERR_RAID_GROUP_ONLY",
        3 => "ERR_RAID_GROUP_FULL",
        4 => "ERR_RAID_GROUP_REQUIREMENTS_UNMATCH",
        _ => return None,
    })
}

/// `UNIT_NPC_FLAGS` `SPIRITGUIDE`, which the area spirit healer's acquire callback tests.
const NPC_FLAG_SPIRITGUIDE: u32 = crate::target::cursor_mode::npc_flags::SPIRITGUIDE;

/// The area spirit healer's aura, cancelled by the reference's healer set and
/// `CancelAreaSpiritHeal`; the reference's aura cancel fires `AREA_SPIRIT_HEALER_OUT_OF_RANGE` for
/// this spell id alone.
pub(crate) const AREA_SPIRIT_HEALER_AURA: u32 = 2584;

/// The acquire radius, squared `[0x9e898c]` = 400 in the enumerate callback `0x523f80`.
const SPIRIT_GUIDE_ACQUIRE_YD: f32 = 20.0;
/// The retain radius, squared `[0xa02d1c]` = 484 in `0x524010`: a healer adopted inside 20 yd is
/// kept until 22.
const SPIRIT_GUIDE_RETAIN_YD: f32 = SPIRIT_GUIDE_ACQUIRE_YD * 1.1;

/// The current-area spirit healer and its wave deadline. Main-thread state the poll, the guide click and `AcceptAreaSpiritHeal` write ([`crate::ui_script::live::MainState`]).
#[derive(Default)]
pub(crate) struct AreaSpiritHealer {
    /// Written only by the healer set, [`Self::store_healer`].
    healer: Option<u64>,
    deadline: Option<Instant>,
}

impl AreaSpiritHealer {
    /// `SMSG_AREA_SPIRIT_HEALER_TIME` after its poll (`0x524b10`): arms the clock only for the held healer and a positive time; whether it did, so `AREA_SPIRIT_HEALER_IN_RANGE` fires.
    pub(crate) fn on_time(&mut self, healer: u64, ms: u32, now: Instant) -> bool {
        let arms = self.healer == Some(healer) && ms as i32 > 0;
        if arms {
            self.deadline = Some(now + std::time::Duration::from_millis(u64::from(ms)));
        }
        arms
    }

    /// The healer set up to its cancel (`0x523eb0`): an unchanged guid returns `None` before anything else, so the poll calls it every frame; a change stores the new healer first (`0x523edd`) and says whether a wave's cancel is owed, its event fired while the deadline still reads (`0x523eea` before `0x523efa`). Without our body (`player` false) the cancel neither fires nor sends (`0x51f75c`), so the wave is zeroed here.
    pub(crate) fn store_healer(&mut self, new: Option<u64>, player: bool) -> Option<bool> {
        // The reference's zero guid is our `None`.
        let new = new.filter(|&g| g != 0);
        if self.healer == new {
            return None;
        }
        self.healer = new;
        let owed = self.deadline.is_some() && player;
        if !owed {
            self.deadline = None;
        }
        Some(owed)
    }

    /// The healer set past its cancel's event: `CMSG_CANCEL_AURA` for the wave cancelled, the deadline zeroed (`0x523efa`), then the query for a non-zero healer (`0x523f02`).
    pub(crate) fn finish_set(&mut self, cancelled: bool, net: Option<&NetCommands>) {
        let send = |cmd| {
            if let Some(net) = net {
                let _ = net.0.send(cmd);
            }
        };
        if cancelled {
            send(ClientCommand::CancelAura {
                spell_id: AREA_SPIRIT_HEALER_AURA,
            });
        }
        self.deadline = None;
        if let Some(healer) = self.healer {
            send(ClientCommand::AreaSpiritHealerQuery { healer });
        }
    }

    /// The whole healer set (`0x523eb0`) from a caller holding our body (the guide click), `fire` running the cancel's event with this state at hand; whether it cancelled a wave.
    #[cfg(test)]
    pub(crate) fn set_healer(
        &mut self,
        new: Option<u64>,
        net: Option<&NetCommands>,
        fire: &mut impl FnMut(&mut Self),
    ) -> bool {
        let Some(cancelled) = self.store_healer(new, true) else {
            return false;
        };
        if cancelled {
            fire(self);
        }
        self.finish_set(cancelled, net);
        cancelled
    }

    /// The spirit-guide click (`0x6db180`): the healer set to 0, then to the guid (`0x6db198`, `0x6db1a7`), a cache-bust that makes the second set query even for the healer already held.
    #[cfg(test)]
    pub(crate) fn click_guide(
        &mut self,
        guid: u64,
        net: Option<&NetCommands>,
        fire: &mut impl FnMut(&mut Self),
    ) {
        self.set_healer(None, net, fire);
        self.set_healer(Some(guid), net, fire);
    }

    /// The held healer: `AcceptAreaSpiritHeal`'s guid and the poll's retain subject.
    pub(crate) fn healer(&self) -> Option<u64> {
        self.healer
    }

    /// `GetAreaSpiritHealerTime` (`0x516b90`).
    pub(crate) fn secs(&self, now: Instant) -> u32 {
        whole_secs(self.deadline, now)
    }
}

/// Drop the held spirit guide when the world does, so no healer, wave or event reaches the next
/// session; the reference clears it only through its healer set, its world-leave path untraced.
fn area_spirit_healer_leave_world(mut spirit: NonSendMut<AreaSpiritHealer>) {
    *spirit = AreaSpiritHealer::default();
}

/// What the healer poll reads: our body and the units the walk visits, and the assist test's tables.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct SpiritPollInputs<'w, 's> {
    me: Query<'w, 's, (&'static ObjectStore, &'static Transform), With<SelfPlayer>>,
    units: Query<
        'w,
        's,
        (
            &'static crate::net::Guid,
            &'static crate::net::NetEntity,
            &'static ObjectStore,
            &'static Transform,
        ),
        Without<SelfPlayer>,
    >,
    factions: Option<Res<'w, crate::target::Factions>>,
    reputations: Option<Res<'w, crate::net::Reputations>>,
    group: Option<Res<'w, crate::ui_party::GroupState>>,
    names: Option<Res<'w, crate::names::NameCache>>,
}

/// The healer poll's first half (`0x524010`): with no body, no ghost (`PLAYER_FLAGS` `0x10`), or the held healer gone or past 22 yd, the healer set to 0 up to its cancel (`0x5240ca`, `0x52409e`); whether that owes the cancel's event, never with no body (`0x51f75c`).
pub(crate) fn poll_drop(spirit: &mut AreaSpiritHealer, inputs: &SpiritPollInputs) -> bool {
    let body = inputs.me.single();
    let drop = match body {
        Err(_) => true,
        Ok((store, _)) if !store.0.player_is_ghost() => true,
        Ok((_, tf)) => spirit.healer().is_some_and(|cached| {
            // A guid that no longer streams drops like one out of range (an object-lookup miss).
            let here = tf.translation;
            !inputs.units.iter().any(|(guid, _, _, unit)| {
                guid.0 == cached
                    && unit.translation.distance_squared(here)
                        <= SPIRIT_GUIDE_RETAIN_YD * SPIRIT_GUIDE_RETAIN_YD
            })
        }),
    };
    drop && spirit.store_healer(None, body.is_ok()) == Some(true)
}

/// The poll's second half, after the drop's event: the drop's set finished, then, a ghost holding no
/// healer, the walk (`0x5240b9`) sets each unit that is a `SPIRITGUIDE`, may assist us (`0x523fc5`,
/// guide to player) and is within 20 yd, in walk order (`0x523ffa`): each a change that queries,
/// the last one held, as the callback `0x523f80` never stops the walk.
pub(crate) fn poll_finish(
    spirit: &mut AreaSpiritHealer,
    inputs: &SpiritPollInputs,
    cancelled: bool,
    net: Option<&NetCommands>,
) {
    if cancelled {
        spirit.finish_set(true, net);
    }
    let Ok((self_store, self_tf)) = inputs.me.single() else {
        return;
    };
    if !self_store.0.player_is_ghost() || spirit.healer().is_some() {
        return;
    }
    let Some(reputations) = inputs.reputations.as_deref() else {
        return;
    };
    let here = self_tf.translation;
    for (guid, kind, store, tf) in inputs.units.iter() {
        if kind.kind != benilla_protocol::EntityKind::Unit {
            continue;
        }
        if store.0.unit_npc_flags() & NPC_FLAG_SPIRITGUIDE == 0 {
            continue;
        }
        if tf.translation.distance_squared(here) > SPIRIT_GUIDE_ACQUIRE_YD * SPIRIT_GUIDE_ACQUIRE_YD
        {
            continue;
        }
        if !crate::target::can_assist_dir(
            crate::target::Assist::UnitToPlayer,
            Some(store),
            inputs.factions.as_deref(),
            reputations,
            crate::target::party_of(inputs.group.as_deref()),
            Some(self_store),
            inputs.names.as_deref(),
        ) {
            continue;
        }
        if let Some(cancelled) = spirit.store_healer(Some(guid.0), true) {
            // Only `on_time` arms a deadline, for the held healer, and none is held here.
            debug_assert!(
                !cancelled,
                "the walk's set found a wave with no healer held"
            );
            spirit.finish_set(cancelled, net);
        }
    }
}

/// The whole poll (`0x524010`), `fire` running the drop's event with the healer at hand; whether it cancelled a wave.
#[cfg(test)]
pub(crate) fn run_poll(
    spirit: &mut AreaSpiritHealer,
    inputs: &SpiritPollInputs,
    net: Option<&NetCommands>,
    fire: impl FnOnce(&mut AreaSpiritHealer),
) -> bool {
    let cancelled = poll_drop(spirit, inputs);
    if cancelled {
        fire(spirit);
    }
    poll_finish(spirit, inputs, cancelled, net);
    cancelled
}

/// What the dialog verbs read at the call: the boot deadline and the poll's inputs.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct DialogView<'w, 's> {
    boot: Option<Res<'w, InstanceBoot>>,
    pub(crate) spirit: SpiritPollInputs<'w, 's>,
    net: Option<Res<'w, NetCommands>>,
}

impl DialogView<'_, '_> {
    /// `GetInstanceBootTimeRemaining` (`0x5162e0`).
    pub(crate) fn instance_boot_secs(&self) -> u32 {
        self.boot.as_ref().map_or(0, |b| b.secs(Instant::now()))
    }

    /// The rest of `AcceptAreaSpiritHeal`'s poll after its drop's event, then the queue naming the healer it left held, none without one (`0x524b60`).
    pub(crate) fn accept(&self, spirit: &mut AreaSpiritHealer, cancelled: bool) {
        let net = self.net.as_deref();
        poll_finish(spirit, &self.spirit, cancelled, net);
        if let (Some(net), Some(healer)) = (net, spirit.healer()) {
            let _ = net.0.send(ClientCommand::AreaSpiritHealerQueue { healer });
        }
    }

    /// Whether our body is held, without which the 2584 aura cancel neither fires nor sends (`0x51f72d`, `0x51f75c`).
    pub(crate) fn has_body(&self) -> bool {
        self.spirit.me.single().is_ok()
    }

    /// `CancelAreaSpiritHeal`'s send after its event (`0x522fa0` -> `0x51f710`): `CMSG_CANCEL_AURA` for 2584, the deadline left as it is.
    pub(crate) fn cancel_area_spirit_heal(&self) {
        if let Some(net) = self.net.as_deref() {
            let _ = net.0.send(ClientCommand::CancelAura {
                spell_id: AREA_SPIRIT_HEALER_AURA,
            });
        }
    }
}

/// The poll run every frame from the world frame's update: its drop's event fires with the
/// `World` lent between the drop and the rest of the poll.
pub(crate) fn poll_area_spirit_healer(world: &mut World) {
    let cancelled = world.run_system_cached(poll_drop_step).unwrap_or(false);
    if cancelled {
        crate::ui_script::safe_world::raise(world, "AREA_SPIRIT_HEALER_OUT_OF_RANGE", vec![]);
    }
    if let Err(e) = world.run_system_cached_with(poll_finish_step, cancelled) {
        warn!("ui_dialog_verbs: the healer poll's finish did not run: {e}");
    }
}

/// The spirit-guide click (`0x6db180`): the healer set to 0, then to the guid (`0x6db198`,
/// `0x6db1a7`), a cache-bust that makes the second set query even for the healer already held;
/// each set's cancel fires its event with the `World` lent while the deadline still reads
/// (`0x523eea` before `0x523efa`).
pub(crate) fn click_guide(world: &mut World, guid: u64) {
    for new in [None, Some(guid)] {
        let Ok(Some(cancelled)) = world.run_system_cached_with(store_healer_step, new) else {
            continue;
        };
        if cancelled {
            crate::ui_script::safe_world::raise(world, "AREA_SPIRIT_HEALER_OUT_OF_RANGE", vec![]);
        }
        if let Err(e) = world.run_system_cached_with(finish_set_step, cancelled) {
            warn!("ui_dialog_verbs: the guide click's set did not finish: {e}");
        }
    }
}

/// The healer set up to its cancel, holding our body (`0x523eb0`).
fn store_healer_step(
    In(new): In<Option<u64>>,
    mut spirit: NonSendMut<AreaSpiritHealer>,
) -> Option<bool> {
    spirit.store_healer(new, true)
}

/// The healer set past its cancel's event (`0x523efa`).
fn finish_set_step(
    In(cancelled): In<bool>,
    mut spirit: NonSendMut<AreaSpiritHealer>,
    net: Option<Res<NetCommands>>,
) {
    spirit.finish_set(cancelled, net.as_deref());
}

/// The poll's drop: whether it cancelled the held healer.
fn poll_drop_step(mut spirit: NonSendMut<AreaSpiritHealer>, inputs: SpiritPollInputs) -> bool {
    poll_drop(&mut spirit, &inputs)
}

/// The poll's rest after its drop's event.
fn poll_finish_step(
    In(cancelled): In<bool>,
    mut spirit: NonSendMut<AreaSpiritHealer>,
    inputs: SpiritPollInputs,
    commands: Res<NetCommands>,
) {
    poll_finish(&mut spirit, &inputs, cancelled, Some(&commands));
}

/// The dialog verbs' packet handlers, each firing its event in its turn.
mod net {
    use benilla_protocol::{SessionEvent, SessionEventKind};
    use bevy::prelude::*;

    use super::{AreaSpiritHealer, InstanceBoot};
    use crate::net::NetHandlerApp;
    use crate::ui_script::live::UiEvents;

    pub(super) fn register(app: &mut App) {
        use SessionEventKind as K;
        app.net_handler(K::RaidGroupOnly, on_raid_group_only)
            .net_handler(K::AreaSpiritHealerTime, on_area_spirit_healer_time);
    }

    /// `SMSG_RAID_GROUP_ONLY`: a zero delay first shows the reason's line (`0x6e3be3`), then the boot setter arms or clears the deadline and fires its event (`0x6e3c10` -> `0x513ad0`).
    fn on_raid_group_only(In(ev): In<SessionEvent>, mut ui: ResMut<UiEvents>) {
        let SessionEvent::RaidGroupOnly { delay_ms, reason } = ev else {
            return;
        };
        if delay_ms == 0 {
            if let Some(key) = super::raid_group_only_key(reason) {
                crate::ui_action::show_in_turn(
                    &mut ui,
                    "ui_dialog_verbs",
                    crate::ui_action::UiError::key(key),
                );
            }
        }
        // The setter runs after the line has shown.
        ui.world(move |world| {
            let Some(mut boot) = world.get_resource_mut::<InstanceBoot>() else {
                return;
            };
            let event = boot.set(delay_ms, std::time::Instant::now());
            if let Some(mut ui) = world.get_resource_mut::<UiEvents>() {
                ui.fire(event, Vec::new());
            }
        });
    }

    /// `SMSG_AREA_SPIRIT_HEALER_TIME` (`0x526a55` -> `0x524b10`): the poll runs first, then the held healer's positive time arms the wave and fires `AREA_SPIRIT_HEALER_IN_RANGE`.
    fn on_area_spirit_healer_time(In(ev): In<SessionEvent>, world: &mut World) {
        let SessionEvent::AreaSpiritHealerTime { healer, ms } = ev else {
            return;
        };
        if let Err(e) = world.run_system_cached(super::poll_area_spirit_healer) {
            warn!("ui_dialog_verbs: the healer poll did not run: {e}");
        }
        let arms = world
            .get_non_send_resource_mut::<AreaSpiritHealer>()
            .is_some_and(|mut s| s.on_time(healer, ms, std::time::Instant::now()));
        if let (true, Some(mut ui)) = (arms, world.get_resource_mut::<UiEvents>()) {
            ui.fire("AREA_SPIRIT_HEALER_IN_RANGE", Vec::new());
        }
    }
}

pub(crate) struct UiDialogVerbsPlugin;

impl Plugin for UiDialogVerbsPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<InstanceBoot>()
            .init_non_send_resource::<AreaSpiritHealer>()
            .add_systems(
                Update,
                // After the mouse pick, the reference's order: the world frame's OnUpdate
                // picks, then polls. Gating is safe: the poll is level-triggered and re-derives
                // the cache every frame.
                poll_area_spirit_healer
                    .after(crate::target::TargetUpdate)
                    .run_if(crate::ui_script::ingame_ui_up)
                    .in_set(crate::char_select::InWorldGated)
                    .in_set(crate::ui_script::UiPaint),
            )
            .add_systems(
                OnExit(crate::char_select::ClientState::InWorld),
                area_spirit_healer_leave_world,
            );
    }
}

/// `CancelAreaSpiritHeal`'s spell, the same 2584 as [`AREA_SPIRIT_HEALER_AURA`].
#[cfg(test)]
const AREA_SPIRIT_HEALER_SPELL: u32 = AREA_SPIRIT_HEALER_AURA;

/// The cancel-aura routine sends nothing when `AttributesEx` has bit 13 set, bit 2 clear and a
/// third test on the player holds; these are the first two terms.
#[cfg(test)]
fn cancel_gate_could_apply(attributes_ex: u32) -> bool {
    attributes_ex & 0x2000 != 0 && attributes_ex & 0x4 == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A command tap: what the healer set sends.
    fn tap() -> (NetCommands, crossbeam_channel::Receiver<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        (NetCommands(tx), rx)
    }

    /// The healer set's sends, in order, by name.
    fn sends(rx: &crossbeam_channel::Receiver<ClientCommand>) -> Vec<String> {
        rx.try_iter()
            .map(|c| match c {
                ClientCommand::CancelAura { spell_id } => format!("cancel {spell_id}"),
                ClientCommand::AreaSpiritHealerQuery { healer } => format!("query {healer}"),
                other => format!("{other:?}"),
            })
            .collect()
    }

    /// The whole healer set, its event recorded as the seconds the deadline still reads.
    fn set(
        spirit: &mut AreaSpiritHealer,
        new: Option<u64>,
        net: &NetCommands,
        now: Instant,
    ) -> Vec<u32> {
        let mut seen = Vec::new();
        spirit.set_healer(new, Some(net), &mut |s| seen.push(s.secs(now)));
        seen
    }

    #[test]
    fn setting_the_same_healer_is_a_complete_no_op() {
        let (net, rx) = tap();
        let mut spirit = AreaSpiritHealer::default();
        let now = Instant::now();
        assert!(set(&mut spirit, Some(7), &net, now).is_empty());
        assert_eq!(sends(&rx), ["query 7"], "a fresh adopt asks for the clock");
        spirit.on_time(7, 30_000, now);
        assert_eq!(spirit.secs(now), 30, "the wave clock is armed");

        assert!(set(&mut spirit, Some(7), &net, now).is_empty());
        assert!(
            sends(&rx).is_empty(),
            "the same guid again: no query, no cancel"
        );
        assert_eq!(
            spirit.secs(now),
            30,
            "and the deadline SURVIVES — the reference's clear sits past the early return"
        );
    }

    /// `0x523eea` runs the cancel, whose event fires at `0x803290`, before `0x523efa` zeroes the
    /// deadline `GetAreaSpiritHealerTime` reads (`0x516b90`): the event's handler reads the wave.
    #[test]
    fn dropping_a_healer_fires_the_cancel_while_the_wave_still_reads() {
        let (net, rx) = tap();
        let mut spirit = AreaSpiritHealer::default();
        let now = Instant::now();
        set(&mut spirit, Some(7), &net, now);
        spirit.on_time(7, 30_500, now);
        let _ = sends(&rx);

        let mut at_event = None;
        let cancelled = spirit.set_healer(None, Some(&net), &mut |s| {
            at_event = Some((s.secs(now), s.healer()));
        });
        assert!(cancelled);
        assert_eq!(
            at_event,
            Some((30, None)),
            "the new healer is stored first (`0x523edd`), the deadline zeroed after"
        );
        assert_eq!(
            sends(&rx),
            ["cancel 2584"],
            "walking away: the aura cancel, and no query for a zero guid"
        );
        assert_eq!(spirit.secs(now), 0, "the clock is zeroed after the event");
        assert_eq!(spirit.healer(), None);

        set(&mut spirit, Some(9), &net, now);
        let _ = sends(&rx);
        assert!(
            set(&mut spirit, None, &net, now).is_empty(),
            "no deadline pending — the reference's `je` skips the cancel"
        );
        assert!(sends(&rx).is_empty());
    }

    #[test]
    fn a_clock_for_another_healer_is_ignored() {
        let (net, _rx) = tap();
        let mut spirit = AreaSpiritHealer::default();
        let now = Instant::now();
        set(&mut spirit, Some(7), &net, now);
        spirit.on_time(8, 30_000, now);
        assert_eq!(spirit.secs(now), 0, "not our healer");
        spirit.on_time(7, 0, now);
        assert_eq!(spirit.secs(now), 0, "a zero time arms nothing");
        spirit.on_time(7, 25_000, now);
        assert_eq!(spirit.secs(now), 25);
    }

    #[test]
    fn clicking_the_cached_guide_still_asks_again() {
        let (net, rx) = tap();
        let mut spirit = AreaSpiritHealer::default();
        let now = Instant::now();
        set(&mut spirit, Some(7), &net, now);
        spirit.on_time(7, 30_000, now);
        let _ = sends(&rx);

        let mut seen = Vec::new();
        spirit.click_guide(7, Some(&net), &mut |s| seen.push(s.secs(now)));
        assert_eq!(
            seen,
            [30],
            "the bust's own leg cancelled the pending wave, its event reading it"
        );
        assert_eq!(
            sends(&rx),
            ["cancel 2584", "query 7"],
            "the bust makes the second set a real change"
        );
        assert_eq!(spirit.healer(), Some(7));
        assert_eq!(spirit.secs(now), 0);
    }

    /// The click's step fires the bust's cancel with the `World` lent while the deadline still
    /// reads (`0x523eea` before `0x523efa`), then finishes the set: the cancel, the query, the
    /// wave zeroed.
    #[test]
    fn the_guide_clicks_step_fires_its_cancel_with_the_wave_still_set() {
        let (net, rx) = tap();
        let mut world = World::new();
        let now = Instant::now();
        let mut spirit = AreaSpiritHealer::default();
        set(&mut spirit, Some(7), &net, now);
        spirit.on_time(7, 30_000, now);
        let _ = sends(&rx);
        world.insert_non_send_resource(spirit);
        world.insert_resource(net);
        let script = benilla_ui::script::UiScript::new().unwrap();
        script
            .run(
                "SEEN = {} local f = CreateFrame('Frame')
                 f:RegisterEvent('AREA_SPIRIT_HEALER_OUT_OF_RANGE')
                 f:SetScript('OnEvent', function() SEEN[#SEEN + 1] = GetAreaSpiritHealerTime() end)",
            )
            .unwrap();
        world.insert_non_send_resource(script);
        click_guide(&mut world, 7);
        let seen = world
            .non_send_resource::<benilla_ui::script::UiScript>()
            .eval::<String>("return table.concat(SEEN, ',')")
            .unwrap();
        let secs: Vec<u32> = seen.split(',').map(|s| s.parse().unwrap()).collect();
        assert_eq!(
            secs.len(),
            1,
            "the bust's first set cancels; the second has no wave"
        );
        assert!(secs[0] > 0, "the listener read the wave still set: {seen}");
        assert_eq!(sends(&rx), ["cancel 2584", "query 7"]);
        let spirit = world.non_send_resource::<AreaSpiritHealer>();
        assert_eq!(spirit.healer(), Some(7));
        assert_eq!(spirit.secs(now), 0);
    }

    /// `UNIT_FIELD_FLAGS`, absolute field 46; `UNIT_FLAG_PVP` (`0x1000`) passes `can_assist`.
    const UNIT_FLAGS_FIELD: u16 = benilla_protocol::field::FIELD_UNIT_FLAGS;
    /// `UNIT_NPC_FLAGS`, absolute field 147.
    const NPC_FLAGS_FIELD: u16 = benilla_protocol::field::FIELD_UNIT_NPC_FLAGS;
    /// `PLAYER_FLAGS`, absolute field 190; bit `0x10` is GHOST.
    const PLAYER_FLAGS_FIELD: u16 = benilla_protocol::field::FIELD_PLAYER_FLAGS;

    fn fields(pairs: &[(u16, u32)]) -> ObjectStore {
        ObjectStore(benilla_protocol::ObjectFields::from_pairs(pairs))
    }

    /// What [`poll_area_spirit_healer`] reads: the body at the origin, one unit `dist` yards along
    /// +X. Faction templates 35 (friendly to all) and 1 (Human) are a real friendly pair, so the
    /// acquire walk needs the shipped catalog; `None` serves the cases refused before the walk.
    fn poll_world(
        ghost: bool,
        guide: bool,
        dist: f32,
        factions: Option<benilla_formats::FactionCatalog>,
    ) -> (World, crossbeam_channel::Receiver<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded::<ClientCommand>();
        let mut world = World::new();
        world.insert_resource(NetCommands(tx));
        world.init_non_send_resource::<AreaSpiritHealer>();
        world.init_resource::<crate::net::Reputations>();
        world.init_resource::<crate::net::GuidIndex>();
        if let Some(catalog) = factions {
            world.insert_resource(crate::target::Factions::from_catalog(catalog));
        }
        world.spawn((
            SelfPlayer,
            fields(&[
                (PLAYER_FLAGS_FIELD, if ghost { 0x10 } else { 0 }),
                (FACTION_TEMPLATE_FIELD, 1),
            ]),
            Transform::from_xyz(0.0, 0.0, 0.0),
        ));
        world.spawn((
            crate::net::Guid(GUIDE),
            crate::net::NetEntity {
                kind: benilla_protocol::EntityKind::Unit,
                display_id: None,
                scale: 1.0,
            },
            fields(&[
                (
                    NPC_FLAGS_FIELD,
                    if guide { NPC_FLAG_SPIRITGUIDE } else { 0 },
                ),
                (UNIT_FLAGS_FIELD, 0x1000),
                (FACTION_TEMPLATE_FIELD, 35),
            ]),
            Transform::from_xyz(dist, 0.0, 0.0),
        ));
        (world, rx)
    }

    /// `UNIT_FIELD_FACTIONTEMPLATE`, absolute field 35.
    const FACTION_TEMPLATE_FIELD: u16 = benilla_protocol::field::FIELD_UNIT_FACTIONTEMPLATE;

    macro_rules! catalog_or_skip {
        () => {{
            let data = benilla_formats::wow_data_or_skip!();
            let mut chain = benilla_formats::open_chain(&data).expect("open chain");
            benilla_formats::load_faction_catalog(&mut chain).expect("FactionTemplate.dbc")
        }};
    }

    const GUIDE: u64 = 0x9111;

    fn poll_once(world: &mut World) {
        use bevy::ecs::system::RunSystemOnce;
        world
            .run_system_once(
                |mut spirit: NonSendMut<AreaSpiritHealer>,
                 inputs: SpiritPollInputs,
                 net: Res<NetCommands>| {
                    run_poll(&mut spirit, &inputs, Some(&net), |_| {});
                },
            )
            .unwrap();
    }

    fn queries(rx: &crossbeam_channel::Receiver<ClientCommand>) -> Vec<u64> {
        rx.try_iter()
            .filter_map(|c| match c {
                ClientCommand::AreaSpiritHealerQuery { healer } => Some(healer),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_living_body_never_adopts_a_spirit_guide() {
        let (mut world, rx) = poll_world(false, true, 1.0, None);
        poll_once(&mut world);
        assert_eq!(world.non_send_resource::<AreaSpiritHealer>().healer(), None);
        assert!(queries(&rx).is_empty(), "no clock is asked for");
    }

    #[test]
    fn a_ghost_adopts_a_guide_inside_the_acquire_radius_and_asks_for_the_clock() {
        let catalog = catalog_or_skip!();
        let (mut world, rx) = poll_world(true, true, 19.0, Some(catalog));
        poll_once(&mut world);
        assert_eq!(
            world.non_send_resource::<AreaSpiritHealer>().healer(),
            Some(GUIDE)
        );
        assert_eq!(
            queries(&rx),
            vec![GUIDE],
            "exactly one query, for that guide"
        );
    }

    #[test]
    fn a_guide_past_the_acquire_radius_is_not_adopted() {
        let (mut world, rx) = poll_world(true, true, 21.0, None);
        poll_once(&mut world);
        assert_eq!(world.non_send_resource::<AreaSpiritHealer>().healer(), None);
        assert!(queries(&rx).is_empty());
    }

    #[test]
    fn an_adopted_guide_is_retained_past_the_acquire_radius_and_dropped_past_the_retain_one() {
        let catalog = catalog_or_skip!();
        let (mut world, rx) = poll_world(true, true, 19.0, Some(catalog));
        poll_once(&mut world);
        assert_eq!(
            world.non_send_resource::<AreaSpiritHealer>().healer(),
            Some(GUIDE)
        );
        let _ = queries(&rx);

        let guide = world
            .query_filtered::<Entity, With<crate::net::Guid>>()
            .iter(&world)
            .next()
            .expect("the guide entity");
        world
            .entity_mut(guide)
            .insert(Transform::from_xyz(21.0, 0.0, 0.0));
        poll_once(&mut world);
        assert_eq!(
            world.non_send_resource::<AreaSpiritHealer>().healer(),
            Some(GUIDE),
            "retained between the two radii"
        );
        assert!(queries(&rx).is_empty(), "a retained guide is not re-asked");

        // 23 yd: outside both.
        world
            .entity_mut(guide)
            .insert(Transform::from_xyz(23.0, 0.0, 0.0));
        poll_once(&mut world);
        assert_eq!(world.non_send_resource::<AreaSpiritHealer>().healer(), None);
    }

    /// The walk's callback sets every guide it passes (`0x523ffa`), so two in reach are each a
    /// change with its own query, in walk order, and the last is held.
    #[test]
    fn each_guide_the_walk_passes_is_set_and_queried_in_turn() {
        const GUIDE2: u64 = 0x9112;
        let catalog = catalog_or_skip!();
        let (mut world, rx) = poll_world(true, true, 5.0, Some(catalog));
        world.spawn((
            crate::net::Guid(GUIDE2),
            crate::net::NetEntity {
                kind: benilla_protocol::EntityKind::Unit,
                display_id: None,
                scale: 1.0,
            },
            fields(&[
                (NPC_FLAGS_FIELD, NPC_FLAG_SPIRITGUIDE),
                (UNIT_FLAGS_FIELD, 0x1000),
                (FACTION_TEMPLATE_FIELD, 35),
            ]),
            Transform::from_xyz(6.0, 0.0, 0.0),
        ));
        let walk: Vec<u64> = world
            .query_filtered::<&crate::net::Guid, Without<SelfPlayer>>()
            .iter(&world)
            .map(|g| g.0)
            .collect();
        poll_once(&mut world);
        assert_eq!(queries(&rx), walk, "one query per guide, in walk order");
        assert_eq!(
            world.non_send_resource::<AreaSpiritHealer>().healer(),
            walk.last().copied(),
            "the last set holds"
        );
        poll_once(&mut world);
        assert!(
            queries(&rx).is_empty(),
            "the held guide is retained, so no walk runs"
        );
    }

    /// With no body the poll still sets the healer to 0 (`0x524036` -> `0x5240ca`), but its cancel
    /// finds no player object and skips the aura cancel (`0x51f75c`): no event, no send, and the
    /// wave zeroed (`0x523efa`).
    #[test]
    fn with_no_body_the_drop_zeroes_the_wave_without_the_cancel() {
        let (mut world, rx) = poll_world(true, true, 5.0, None);
        let now = Instant::now();
        {
            let net = NetCommands(world.resource::<NetCommands>().0.clone());
            let mut spirit = world.non_send_resource_mut::<AreaSpiritHealer>();
            spirit.set_healer(Some(GUIDE), Some(&net), &mut |_| {});
            spirit.on_time(GUIDE, 30_500, now);
        }
        let _ = sends(&rx);
        let me = world
            .query_filtered::<Entity, With<SelfPlayer>>()
            .iter(&world)
            .next()
            .expect("the body");
        world.despawn(me);

        use bevy::ecs::system::RunSystemOnce;
        let fired = world
            .run_system_once(
                |mut spirit: NonSendMut<AreaSpiritHealer>,
                 inputs: SpiritPollInputs,
                 net: Res<NetCommands>| {
                    let mut fired = 0;
                    let cancelled = run_poll(&mut spirit, &inputs, Some(&net), |_| fired += 1);
                    (fired, cancelled)
                },
            )
            .unwrap();
        assert_eq!(
            fired,
            (0, false),
            "no player object: the cancel's event never fires"
        );
        assert!(sends(&rx).is_empty(), "and no CMSG_CANCEL_AURA goes out");
        let spirit = world.non_send_resource::<AreaSpiritHealer>();
        assert_eq!(spirit.healer(), None, "the healer is still set to 0");
        assert_eq!(spirit.secs(now), 0, "and the wave is zeroed");
    }

    #[test]
    fn a_unit_without_the_spiritguide_flag_is_never_adopted() {
        let (mut world, rx) = poll_world(true, false, 1.0, None);
        poll_once(&mut world);
        assert_eq!(world.non_send_resource::<AreaSpiritHealer>().healer(), None);
        assert!(queries(&rx).is_empty());
    }

    #[test]
    fn losing_the_ghost_state_drops_the_guide() {
        let catalog = catalog_or_skip!();
        let (mut world, _rx) = poll_world(true, true, 5.0, Some(catalog));
        poll_once(&mut world);
        assert_eq!(
            world.non_send_resource::<AreaSpiritHealer>().healer(),
            Some(GUIDE)
        );

        let me = world
            .query_filtered::<Entity, With<SelfPlayer>>()
            .iter(&world)
            .next()
            .expect("the body");
        world
            .entity_mut(me)
            .insert(fields(&[(PLAYER_FLAGS_FIELD, 0)]));
        poll_once(&mut world);
        assert_eq!(
            world.non_send_resource::<AreaSpiritHealer>().healer(),
            None,
            "alive again: the cache clears even though the guide has not moved"
        );
    }

    #[test]
    fn spell_2584_never_trips_the_cancel_gate() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let catalog = benilla_formats::load_spell_catalog(&mut chain).expect("Spell.dbc");
        let ex = catalog
            .get(AREA_SPIRIT_HEALER_SPELL)
            .map(|d| d.attributes_ex)
            .expect("spell 2584 in Spell.dbc");
        assert!(
            !cancel_gate_could_apply(ex),
            "spell 2584 AttributesEx = {ex:#x}: the gate's third leg would decide, and it is open"
        );
    }

    #[test]
    fn the_boot_clock_arms_on_a_delay_and_clears_otherwise() {
        let mut boot = InstanceBoot::default();
        let now = Instant::now();
        assert_eq!(boot.set(60_500, now), "INSTANCE_BOOT_START");
        assert_eq!(boot.secs(now), 60, "whole seconds, truncated");
        assert_eq!(boot.set(0, now), "INSTANCE_BOOT_STOP");
        assert_eq!(boot.secs(now), 0);
        assert_eq!(
            boot.set(0x8000_0000, now),
            "INSTANCE_BOOT_STOP",
            "a negative delay clears"
        );
        for (reason, key) in [
            (1, "ERR_RAID_GROUP_LOWLEVEL"),
            (2, "ERR_RAID_GROUP_ONLY"),
            (3, "ERR_RAID_GROUP_FULL"),
            (4, "ERR_RAID_GROUP_REQUIREMENTS_UNMATCH"),
        ] {
            assert_eq!(raid_group_only_key(reason), Some(key));
        }
        assert_eq!(
            raid_group_only_key(7),
            None,
            "a reason outside 1-4 names nothing"
        );
    }

    #[test]
    fn the_spirit_healer_clock_only_arms_for_the_cached_healer() {
        let mut spirit = AreaSpiritHealer::default();
        let now = Instant::now();
        assert!(
            !spirit.on_time(0x77, 30_000, now),
            "no healer held: ignored"
        );
        spirit.healer = Some(0x77);
        assert!(!spirit.on_time(0x78, 30_000, now), "another guid: ignored");
        assert!(!spirit.on_time(0x77, 0, now), "a zero time: ignored");
        assert!(spirit.on_time(0x77, 30_000, now));
        assert_eq!(spirit.secs(now), 30);
    }
}
