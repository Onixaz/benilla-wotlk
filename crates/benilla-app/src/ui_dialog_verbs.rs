//! The app half of the verbs behind the stock `StaticPopup.lua` dialogs: the packet feeds that
//! raise them and the drains behind their buttons, for the pet trainer's unlearn, the instance
//! boot countdown and the area spirit healer's wave; the battleground queue is
//! `crate::ui_battlefield`'s.

use std::time::Instant;

use benilla_ui::script::{ScriptValue, UiScript};
use bevy::ecs::system::NonSendMut;
use bevy::prelude::*;

use crate::net::{ClientCommand, NetCommands, ObjectStore, SelfPlayer};
use crate::ui_script::{UiFeed, UiInput};
use crate::ui_session::{close_npc_session_out_of_range, NpcSession};

/// The pet trainer's pending question: the latch and the cost.
#[derive(Resource, Default)]
pub(crate) struct PetUnlearnState {
    npc: Option<u64>,
    cost: u32,
    ask: bool,
}

impl PetUnlearnState {
    /// The inbound `SMSG_PET_UNLEARN_CONFIRM`: latch and owe the dialog.
    pub(crate) fn ask(&mut self, npc: u64, cost: u32) {
        self.npc = Some(npc);
        self.cost = cost;
        self.ask = true;
    }

    fn pending(&self) -> Option<u64> {
        self.npc
    }
}

impl NpcSession for PetUnlearnState {
    fn npc(&self) -> Option<u64> {
        self.npc
    }
    fn close(&mut self) {
        self.npc = None;
        self.cost = 0;
        self.ask = false;
    }
}

/// The instance-boot clock, and what the last packet owes the UI.
#[derive(Resource, Default)]
pub(crate) struct InstanceBoot {
    deadline: Option<Instant>,
    events: Vec<&'static str>,
    errors: Vec<&'static str>,
}

impl InstanceBoot {
    /// `SMSG_RAID_GROUP_ONLY`: a zero delay first shows the reason's error (the jump table at
    /// `0x6e3be3`), then the boot clock (`0x513ad0`) arms on a positive delay as `i32` and clears
    /// otherwise; an event fires either way.
    pub(crate) fn apply(&mut self, delay_ms: u32, reason: u32, now: Instant) {
        if delay_ms == 0 {
            match reason {
                1 => self.errors.push("ERR_RAID_GROUP_LOWLEVEL"),
                2 => self.errors.push("ERR_RAID_GROUP_ONLY"),
                3 => self.errors.push("ERR_RAID_GROUP_FULL"),
                4 => self.errors.push("ERR_RAID_GROUP_REQUIREMENTS_UNMATCH"),
                _ => {}
            }
        }
        if delay_ms as i32 > 0 {
            self.deadline = Some(now + std::time::Duration::from_millis(u64::from(delay_ms)));
            self.events.push("INSTANCE_BOOT_START");
        } else {
            self.deadline = None;
            self.events.push("INSTANCE_BOOT_STOP");
        }
    }

    /// Whole seconds left, 0 when idle or past, as the reference divides a clamped ms remainder.
    pub(crate) fn secs(&self, now: Instant) -> u32 {
        self.deadline
            .map(|d| d.saturating_duration_since(now).as_secs())
            .map_or(0, |s| u32::try_from(s).unwrap_or(u32::MAX))
    }
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

/// What [`AreaSpiritHealer::set_healer`] owes the frame; the reference's healer set does both.
#[derive(Default, PartialEq, Eq, Debug)]
pub(crate) struct SetHealerOutcome {
    /// A wave was pending and the healer changed: `AREA_SPIRIT_HEALER_OUT_OF_RANGE` fires and
    /// `CMSG_CANCEL_AURA` goes out for 2584; this, not Lua, closes the dialog.
    pub(crate) cancel_aura: bool,
    /// A newly adopted non-zero healer: `CMSG_AREA_SPIRIT_HEALER_QUERY` with it.
    pub(crate) query: Option<u64>,
}

/// The current-area spirit healer and its wave clock.
#[derive(Resource, Default)]
pub(crate) struct AreaSpiritHealer {
    /// Written only by [`Self::set_healer`], the reference's healer set.
    healer: Option<u64>,
    deadline: Option<Instant>,
    in_range: bool,
}

impl AreaSpiritHealer {
    /// `SMSG_AREA_SPIRIT_HEALER_TIME`: arms the clock only for the cached healer and a positive
    /// time, and owes `AREA_SPIRIT_HEALER_IN_RANGE`.
    pub(crate) fn on_time(&mut self, healer: u64, ms: u32, now: Instant) {
        if self.healer == Some(healer) && ms > 0 {
            self.deadline = Some(now + std::time::Duration::from_millis(u64::from(ms)));
            self.in_range = true;
        }
    }

    /// An unchanged guid returns before anything else, so the poll calls it every frame; a change
    /// cancels a pending wave, zeroes the deadline and queries a non-zero healer.
    pub(crate) fn set_healer(&mut self, new: Option<u64>) -> SetHealerOutcome {
        // The reference's zero guid is our `None`.
        let new = new.filter(|&g| g != 0);
        if self.healer == new {
            return SetHealerOutcome::default();
        }
        self.healer = new;
        let cancel_aura = self.deadline.take().is_some();
        if cancel_aura {
            // The cancel closes the dialog, so an `_IN_RANGE` still owed is stale.
            self.in_range = false;
        }
        SetHealerOutcome {
            cancel_aura,
            query: new,
        }
    }

    /// The spirit-guide click: the reference sets the healer to 0, then to the guid, a cache-bust
    /// that makes the second call query even for the healer already cached.
    pub(crate) fn click_guide(&mut self, guid: u64) -> SetHealerOutcome {
        let bust = self.set_healer(None);
        let set = self.set_healer(Some(guid));
        SetHealerOutcome {
            cancel_aura: bust.cancel_aura || set.cancel_aura,
            query: set.query,
        }
    }

    /// The cached healer: `AcceptAreaSpiritHeal`'s guid and the poll's retain subject.
    pub(crate) fn healer(&self) -> Option<u64> {
        self.healer
    }

    fn secs(&self, now: Instant) -> u32 {
        self.deadline
            .map(|d| d.saturating_duration_since(now).as_secs())
            .map_or(0, |s| u32::try_from(s).unwrap_or(u32::MAX))
    }
}

/// Drop the cached spirit guide when the world does, so no healer, wave or event reaches the next
/// session; the reference clears it only through its healer set, its world-leave path untraced.
fn area_spirit_healer_leave_world(mut spirit: ResMut<AreaSpiritHealer>) {
    *spirit = AreaSpiritHealer::default();
}

pub(crate) fn feed_dialog_verbs(
    script: Option<NonSendMut<UiScript>>,
    mut pet: ResMut<PetUnlearnState>,
    mut boot: ResMut<InstanceBoot>,
    mut spirit: ResMut<AreaSpiritHealer>,
    mut sink: crate::ui_action::MessageSink,
) {
    let Some(mut script) = script else {
        return;
    };
    let now = Instant::now();

    script.set_pet_untrainer_pending(pet.pending().is_some());
    if pet.ask {
        pet.ask = false;
        script.fire_event(
            "CONFIRM_PET_UNLEARN",
            vec![ScriptValue::Int(i64::from(pet.cost))],
        );
    }

    script.set_instance_boot_secs(boot.secs(now));
    for event in std::mem::take(&mut boot.events) {
        script.fire_event(event, vec![]);
    }
    let lines: Vec<_> = std::mem::take(&mut boot.errors)
        .into_iter()
        .filter_map(|key| crate::ui_action::keyed_line(&script, key))
        .collect();
    if !lines.is_empty() {
        crate::ui_action::show_messages(&mut script, &mut sink, "ui_dialog_verbs", lines);
    }

    let secs = spirit.secs(now);
    script.set_area_spirit_healer(spirit.healer().is_some(), secs);
    if std::mem::take(&mut spirit.in_range) {
        script.fire_event("AREA_SPIRIT_HEALER_IN_RANGE", vec![]);
    }
}

/// The pet trainer's confirm and the spirit healer's accept, the two drains over a latch.
fn drain_latch_verbs(
    script: Option<NonSendMut<UiScript>>,
    pet: Res<PetUnlearnState>,
    spirit: Res<AreaSpiritHealer>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    commands: Res<NetCommands>,
    mut sink: crate::ui_action::MessageSink,
) {
    let Some(mut script) = script else {
        return;
    };

    // ConfirmPetUnlearn: `cost > coinage` shows ERR_NOT_ENOUGH_MONEY and sends nothing.
    let confirms = script.take_pet_unlearn_confirms();
    if confirms > 0 {
        if let Some(npc) = pet.pending() {
            let money = self_q
                .single()
                .ok()
                .and_then(|store| store.0.player_money())
                .unwrap_or(0);
            if pet.cost > money {
                if let Some(line) = crate::ui_action::keyed_line(&script, "ERR_NOT_ENOUGH_MONEY") {
                    crate::ui_action::show_messages(
                        &mut script,
                        &mut sink,
                        "ui_dialog_verbs",
                        [line],
                    );
                }
            } else {
                for _ in 0..confirms {
                    let _ = commands.0.send(ClientCommand::PetUnlearn { trainer: npc });
                }
            }
        }
    }

    // AcceptAreaSpiritHeal: the cached healer's guid, nothing without one.
    let accepts = script.take_area_spirit_accepts();
    if let Some(healer) = spirit.healer() {
        for _ in 0..accepts {
            let _ = commands
                .0
                .send(ClientCommand::AreaSpiritHealerQueue { healer });
        }
    }
}

/// The area-spirit-healer poll (`0x524010`), run every frame from the world frame's update: only a
/// ghost (`PLAYER_FLAGS` `0x10`) holds a healer; a cached one is kept within 22 yd, else the last
/// unit in the walk that is a `SPIRITGUIDE`, may assist us (`0x523fc5`, guide → player) and is
/// within 20 yd is adopted. Last, not nearest: the enumerate callback `0x523f80` never stops the
/// walk.
fn poll_area_spirit_healer(
    mut spirit: ResMut<AreaSpiritHealer>,
    me: Query<(&ObjectStore, &Transform), With<SelfPlayer>>,
    units: Query<
        (
            &crate::net::Guid,
            &crate::net::NetEntity,
            &ObjectStore,
            &Transform,
        ),
        Without<SelfPlayer>,
    >,
    factions: Option<Res<crate::target::Factions>>,
    reputations: Res<crate::net::Reputations>,
    group: Option<Res<crate::ui_party::GroupState>>,
    names: Option<Res<crate::names::NameCache>>,
    commands: Res<NetCommands>,
    mut script: Option<NonSendMut<UiScript>>,
) {
    let mut apply = |outcome: SetHealerOutcome| {
        if outcome.cancel_aura {
            // The event with no argument, the packet with no guid.
            if let Some(script) = script.as_deref_mut() {
                script.fire_event("AREA_SPIRIT_HEALER_OUT_OF_RANGE", vec![]);
            }
            let _ = commands.0.send(ClientCommand::CancelAura {
                spell_id: AREA_SPIRIT_HEALER_AURA,
            });
        }
        if let Some(healer) = outcome.query {
            let _ = commands
                .0
                .send(ClientCommand::AreaSpiritHealerQuery { healer });
        }
    };

    let Ok((self_store, self_tf)) = me.single() else {
        apply(spirit.set_healer(None));
        return;
    };
    if !self_store.0.player_is_ghost() {
        apply(spirit.set_healer(None));
        return;
    }
    let here = self_tf.translation;

    // Retain: a guid that no longer streams drops like one out of range (an object-lookup miss).
    if let Some(cached) = spirit.healer() {
        let still = units.iter().find(|(guid, ..)| guid.0 == cached);
        let keep = still.is_some_and(|(_, _, _, tf)| {
            tf.translation.distance_squared(here) <= SPIRIT_GUIDE_RETAIN_YD * SPIRIT_GUIDE_RETAIN_YD
        });
        if !keep {
            apply(spirit.set_healer(None));
        }
    }
    if spirit.healer().is_some() {
        return;
    }

    // Acquire: `0x523f80` asks the guide whether it may assist us (`0x7293d0`).
    let mut adopted = None;
    for (guid, kind, store, tf) in units.iter() {
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
            factions.as_deref(),
            &reputations,
            crate::target::party_of(group.as_deref()),
            Some(self_store),
            names.as_deref(),
        ) {
            continue;
        }
        adopted = Some(guid.0); // last wins: the callback never stops the walk
    }
    if let Some(guid) = adopted {
        apply(spirit.set_healer(Some(guid)));
    }
}

/// The dialog verbs' packet handlers: each parks a question or a countdown for the feed.
mod net {
    use benilla_protocol::{SessionEvent, SessionEventKind};
    use bevy::prelude::*;

    use super::{AreaSpiritHealer, InstanceBoot, PetUnlearnState};
    use crate::net::NetHandlerApp;

    pub(super) fn register(app: &mut App) {
        use SessionEventKind as K;
        app.net_handler(K::PetUnlearnConfirm, on_pet_unlearn_confirm)
            .net_handler(K::RaidGroupOnly, on_raid_group_only)
            .net_handler(K::AreaSpiritHealerTime, on_area_spirit_healer_time);
    }

    /// The pet trainer's question; a zero guid shows `ERR_TALENT_WIPE_ERROR` instead.
    fn on_pet_unlearn_confirm(
        In(ev): In<SessionEvent>,
        mut unlearn: ResMut<PetUnlearnState>,
        mut errors: ResMut<crate::ui_action::UiErrorKeys>,
    ) {
        if let SessionEvent::PetUnlearnConfirm { trainer, cost } = ev {
            if trainer == 0 {
                debug!("net: pet unlearn refused (zero trainer) — no dialog");
                errors
                    .0
                    .push(crate::ui_action::UiError::key("ERR_TALENT_WIPE_ERROR"));
            } else {
                debug!("net: trainer {trainer:#x} asks to unlearn the pet for {cost} copper");
                unlearn.ask(trainer, cost);
            }
        }
    }

    fn on_raid_group_only(In(ev): In<SessionEvent>, mut boot: ResMut<InstanceBoot>) {
        if let SessionEvent::RaidGroupOnly { delay_ms, reason } = ev {
            boot.apply(delay_ms, reason, std::time::Instant::now());
        }
    }

    fn on_area_spirit_healer_time(In(ev): In<SessionEvent>, mut spirit: ResMut<AreaSpiritHealer>) {
        if let SessionEvent::AreaSpiritHealerTime { healer, ms } = ev {
            spirit.on_time(healer, ms, std::time::Instant::now());
        }
    }
}

pub(crate) struct UiDialogVerbsPlugin;

impl Plugin for UiDialogVerbsPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<PetUnlearnState>()
            .init_resource::<InstanceBoot>()
            .init_resource::<AreaSpiritHealer>()
            .add_systems(
                Update,
                (
                    close_npc_session_out_of_range::<PetUnlearnState>.before(feed_dialog_verbs),
                    // Gated on the in-game interface: vmangos can send `SMSG_RAID_GROUP_ONLY` in
                    // the login burst (`Player.cpp:18686`), and an `INSTANCE_BOOT_START` fired at
                    // the boot VM would be lost; the queue waits instead.
                    feed_dialog_verbs
                        .in_set(UiFeed)
                        .run_if(crate::ui_script::ingame_ui_up),
                    // After the mouse pick, the reference's order: the world frame's OnUpdate
                    // picks, then polls. Gating is safe: the poll is level-triggered and re-derives
                    // the cache every frame.
                    poll_area_spirit_healer
                        .after(crate::target::TargetUpdate)
                        .run_if(crate::ui_script::ingame_ui_up)
                        .in_set(crate::char_select::InWorldGated),
                    // Before the target chain, where the poll and the click can change the
                    // healer: the reference resolves an Accept press in the UI dispatch, before
                    // its pick and poll run later in the frame.
                    drain_latch_verbs
                        .after(UiInput)
                        .before(crate::target::TargetUpdate),
                ),
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

    #[test]
    fn setting_the_same_healer_is_a_complete_no_op() {
        let mut spirit = AreaSpiritHealer::default();
        assert_eq!(
            spirit.set_healer(Some(7)),
            SetHealerOutcome {
                cancel_aura: false,
                query: Some(7)
            },
            "a fresh adopt asks for the clock"
        );
        let now = Instant::now();
        spirit.on_time(7, 30_000, now);
        assert_eq!(spirit.secs(now), 30, "the wave clock is armed");

        assert_eq!(
            spirit.set_healer(Some(7)),
            SetHealerOutcome::default(),
            "the same guid again: no query, no cancel"
        );
        assert_eq!(
            spirit.secs(now),
            30,
            "and the deadline SURVIVES — the reference's clear sits past the early return"
        );
    }

    #[test]
    fn dropping_a_healer_cancels_the_wave_and_sends_nothing() {
        let mut spirit = AreaSpiritHealer::default();
        spirit.set_healer(Some(7));
        let now = Instant::now();
        spirit.on_time(7, 30_000, now);

        assert_eq!(
            spirit.set_healer(None),
            SetHealerOutcome {
                cancel_aura: true,
                query: None
            },
            "walking away: the aura cancel, and no query for a zero guid"
        );
        assert_eq!(spirit.secs(now), 0, "the clock is zeroed");
        assert_eq!(spirit.healer(), None);

        spirit.set_healer(Some(9));
        assert_eq!(
            spirit.set_healer(None),
            SetHealerOutcome {
                cancel_aura: false,
                query: None
            },
            "no deadline pending — the reference's `je` skips the cancel"
        );
    }

    #[test]
    fn a_clock_for_another_healer_is_ignored() {
        let mut spirit = AreaSpiritHealer::default();
        spirit.set_healer(Some(7));
        let now = Instant::now();
        spirit.on_time(8, 30_000, now);
        assert_eq!(spirit.secs(now), 0, "not our healer");
        spirit.on_time(7, 0, now);
        assert_eq!(spirit.secs(now), 0, "a zero time arms nothing");
        spirit.on_time(7, 25_000, now);
        assert_eq!(spirit.secs(now), 25);
    }

    #[test]
    fn clicking_the_cached_guide_still_asks_again() {
        let mut spirit = AreaSpiritHealer::default();
        spirit.set_healer(Some(7));
        let now = Instant::now();
        spirit.on_time(7, 30_000, now);

        let outcome = spirit.click_guide(7);
        assert_eq!(
            outcome.query,
            Some(7),
            "the bust makes the second call a real change"
        );
        assert!(
            outcome.cancel_aura,
            "and the bust's own leg cancelled the pending wave"
        );
        assert_eq!(spirit.healer(), Some(7));
    }

    /// `UNIT_FIELD_FLAGS`, absolute field 46; `UNIT_FLAG_PVP` (`0x1000`) passes `can_assist`.
    const UNIT_FLAGS_FIELD: u16 = 46;
    /// `UNIT_NPC_FLAGS`, absolute field 147.
    const NPC_FLAGS_FIELD: u16 = 147;
    /// `PLAYER_FLAGS`, absolute field 190; bit `0x10` is GHOST.
    const PLAYER_FLAGS_FIELD: u16 = 190;

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
        world.init_resource::<AreaSpiritHealer>();
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
    const FACTION_TEMPLATE_FIELD: u16 = 35;

    macro_rules! catalog_or_skip {
        () => {{
            let data = benilla_formats::wow_data_or_skip!();
            let mut chain = benilla_formats::open_chain(&data).expect("open chain");
            benilla_formats::load_faction_catalog(&mut chain).expect("FactionTemplate.dbc")
        }};
    }

    const GUIDE: u64 = 0x9111;

    fn run_poll(world: &mut World) {
        use bevy::ecs::system::RunSystemOnce;
        world.run_system_once(poll_area_spirit_healer).unwrap();
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
        run_poll(&mut world);
        assert_eq!(world.resource::<AreaSpiritHealer>().healer(), None);
        assert!(queries(&rx).is_empty(), "no clock is asked for");
    }

    #[test]
    fn a_ghost_adopts_a_guide_inside_the_acquire_radius_and_asks_for_the_clock() {
        let catalog = catalog_or_skip!();
        let (mut world, rx) = poll_world(true, true, 19.0, Some(catalog));
        run_poll(&mut world);
        assert_eq!(world.resource::<AreaSpiritHealer>().healer(), Some(GUIDE));
        assert_eq!(
            queries(&rx),
            vec![GUIDE],
            "exactly one query, for that guide"
        );
    }

    #[test]
    fn a_guide_past_the_acquire_radius_is_not_adopted() {
        let (mut world, rx) = poll_world(true, true, 21.0, None);
        run_poll(&mut world);
        assert_eq!(world.resource::<AreaSpiritHealer>().healer(), None);
        assert!(queries(&rx).is_empty());
    }

    #[test]
    fn an_adopted_guide_is_retained_past_the_acquire_radius_and_dropped_past_the_retain_one() {
        let catalog = catalog_or_skip!();
        let (mut world, rx) = poll_world(true, true, 19.0, Some(catalog));
        run_poll(&mut world);
        assert_eq!(world.resource::<AreaSpiritHealer>().healer(), Some(GUIDE));
        let _ = queries(&rx);

        let guide = world
            .query_filtered::<Entity, With<crate::net::Guid>>()
            .iter(&world)
            .next()
            .expect("the guide entity");
        world
            .entity_mut(guide)
            .insert(Transform::from_xyz(21.0, 0.0, 0.0));
        run_poll(&mut world);
        assert_eq!(
            world.resource::<AreaSpiritHealer>().healer(),
            Some(GUIDE),
            "retained between the two radii"
        );
        assert!(queries(&rx).is_empty(), "a retained guide is not re-asked");

        // 23 yd: outside both.
        world
            .entity_mut(guide)
            .insert(Transform::from_xyz(23.0, 0.0, 0.0));
        run_poll(&mut world);
        assert_eq!(world.resource::<AreaSpiritHealer>().healer(), None);
    }

    #[test]
    fn a_unit_without_the_spiritguide_flag_is_never_adopted() {
        let (mut world, rx) = poll_world(true, false, 1.0, None);
        run_poll(&mut world);
        assert_eq!(world.resource::<AreaSpiritHealer>().healer(), None);
        assert!(queries(&rx).is_empty());
    }

    #[test]
    fn losing_the_ghost_state_drops_the_guide() {
        let catalog = catalog_or_skip!();
        let (mut world, _rx) = poll_world(true, true, 5.0, Some(catalog));
        run_poll(&mut world);
        assert_eq!(world.resource::<AreaSpiritHealer>().healer(), Some(GUIDE));

        let me = world
            .query_filtered::<Entity, With<SelfPlayer>>()
            .iter(&world)
            .next()
            .expect("the body");
        world
            .entity_mut(me)
            .insert(fields(&[(PLAYER_FLAGS_FIELD, 0)]));
        run_poll(&mut world);
        assert_eq!(
            world.resource::<AreaSpiritHealer>().healer(),
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
    fn the_boot_clock_arms_on_a_delay_and_clears_on_zero_with_the_reason() {
        let mut boot = InstanceBoot::default();
        let now = Instant::now();
        boot.apply(60_500, 0, now);
        assert_eq!(boot.events, vec!["INSTANCE_BOOT_START"]);
        assert_eq!(boot.secs(now), 60, "whole seconds, truncated");
        boot.apply(0, 2, now);
        assert_eq!(
            boot.events,
            vec!["INSTANCE_BOOT_START", "INSTANCE_BOOT_STOP"]
        );
        assert_eq!(boot.errors, vec!["ERR_RAID_GROUP_ONLY"]);
        assert_eq!(boot.secs(now), 0);
        for (reason, key) in [
            (1, "ERR_RAID_GROUP_LOWLEVEL"),
            (3, "ERR_RAID_GROUP_FULL"),
            (4, "ERR_RAID_GROUP_REQUIREMENTS_UNMATCH"),
        ] {
            boot.apply(0, reason, now);
            assert_eq!(boot.errors.last(), Some(&key));
        }
        boot.apply(0, 7, now);
        assert_eq!(boot.errors.len(), 4, "a reason outside 1-4 names nothing");
        boot.apply(0x8000_0000, 0, now);
        assert_eq!(boot.secs(now), 0, "a negative delay clears");
        assert_eq!(boot.errors.len(), 4, "only a zero delay shows a reason");
    }

    #[test]
    fn the_spirit_healer_clock_only_arms_for_the_cached_healer() {
        let mut spirit = AreaSpiritHealer::default();
        let now = Instant::now();
        spirit.on_time(0x77, 30_000, now);
        assert!(!spirit.in_range, "no healer cached: the packet is ignored");
        spirit.healer = Some(0x77);
        spirit.on_time(0x78, 30_000, now);
        assert!(!spirit.in_range, "another guid: ignored");
        spirit.on_time(0x77, 0, now);
        assert!(!spirit.in_range, "a zero time: ignored");
        spirit.on_time(0x77, 30_000, now);
        assert!(spirit.in_range);
        assert_eq!(spirit.secs(now), 30);
    }

    #[test]
    fn the_pet_question_latches_and_closes_like_the_talent_wipe() {
        let mut pet = PetUnlearnState::default();
        pet.ask(0x2b, 10_000);
        assert_eq!(pet.pending(), Some(0x2b));
        assert!(pet.ask);
        pet.close();
        assert_eq!(pet.pending(), None);
        assert_eq!(pet.cost, 0);
    }
}
