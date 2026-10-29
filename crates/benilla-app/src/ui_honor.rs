//! The honor feed: our own 3.3.5a honor fields become the snapshot the PvP window's honor pane
//! reads, and the inspect-honor reply is matched to the inspected player. The honor block streams
//! only to its owner, so another player's numbers come only from `MSG_INSPECT_HONOR_STATS`; the
//! stock pane re-asks from its `OnShow` while it holds nothing, the only retry.
//!
//! The reference fires the pane's events from field watches (`0x6e5180`'s list): today's kills,
//! today's honor and the lifetime kills fire `PLAYER_PVP_KILLS_CHANGED`, `PLAYER_BYTES_3` fires
//! `PLAYER_PVP_RANK_CHANGED`, and either currency fires `HONOR_CURRENCY_UPDATE` and
//! `CURRENCY_DISPLAY_UPDATE` (`0x6da3c0`, `0x6da430`). Yesterday's numbers have no watch.
//!
//! Deviation: a currency gain's `LOOTWINDOWCOINSOUND` is not played, as no sound kit is keyed by
//! name here.

use std::collections::HashMap;

use bevy::prelude::*;

use benilla_ui::script::{HonorState, InspectHonorData, ScriptValue, UiScript};

use crate::net::{ClientCommand, NetCommands, ObjectStore, SelfPlayer};
use crate::ui_script::{UiFeed, VmMemo};

/// The inspect-honor reply held: one slot, like the reference's `HasInspectHonorData` latch.
#[derive(Resource, Default)]
pub(crate) struct InspectHonor(pub(crate) Option<benilla_protocol::messages::InspectHonorStats>);

/// What the last push told this VM, keyed on the VM so a `/reload`'s fresh VM is told again.
#[derive(Resource, Default)]
struct HonorFeedState {
    vm: VmMemo<HonorFeedMemo>,
}

/// The per-VM change bases: the last snapshot and the held reply's guid.
#[derive(Default)]
struct HonorFeedMemo {
    last: Option<HonorState>,
    last_inspect: Option<u64>,
}

/// The inspect-honor reply's handler; the honor award, `SMSG_PVP_CREDIT`, is the chat family's.
mod net {
    use benilla_protocol::{SessionEvent, SessionEventKind};
    use bevy::prelude::*;

    use super::InspectHonor;
    use crate::net::NetHandlerApp;

    pub(super) fn register(app: &mut App) {
        app.net_handler(SessionEventKind::InspectHonorStats, on_inspect_stats);
    }

    /// A reply replaces whatever is held, whoever it is for: the reference's latch is one slot.
    fn on_inspect_stats(In(ev): In<SessionEvent>, mut inspect: ResMut<InspectHonor>) {
        if let SessionEvent::InspectHonorStats(stats) = ev {
            inspect.0 = Some(stats);
        }
    }
}

pub(crate) struct UiHonorPlugin;

impl Plugin for UiHonorPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<InspectHonor>()
            .init_resource::<HonorFeedState>()
            .init_resource::<crate::nameplates::PvpNameStrings>()
            .add_systems(Update, (feed_honor, feed_pvp_timer).in_set(UiFeed))
            // A VM holder, so after the tick; before the driver, so a new VM's strings reach the
            // lines the frame it arrives.
            .add_systems(
                Update,
                feed_pvp_name_strings
                    .after(crate::ui_script::UiInput)
                    .before(crate::nameplates::drive_nameplates),
            );
    }
}

/// `PLAYER_FLAGS_PVP_DESIRED` and `PLAYER_FLAGS_PVP_TIMER`.
const PVP_DESIRED: u32 = 0x200;
const PVP_TIMER: u32 = benilla_ui::script::PLAYER_FLAGS_PVP_TIMER;

/// The PvP toggle timer's five minutes (`0x6de789`).
const PVP_TIMER_SPAN: std::time::Duration = std::time::Duration::from_millis(300_000);

/// The PvP cooldown's timestamp (`[player+0x1940]`) and the flags it was last set from.
#[derive(Default)]
struct PvpTimer {
    flags: Option<u32>,
    stamp: Option<std::time::Instant>,
}

impl PvpTimer {
    /// World entry (`0x6e7f50`): PvP desired clears the stamp, no timer flag starts it, a running
    /// timer keeps the zeroed stamp.
    fn enter(&mut self, flags: u32, now: std::time::Instant) {
        if flags & PVP_DESIRED != 0 {
            self.stamp = None;
        } else if flags & PVP_TIMER == 0 {
            self.stamp = Some(now + PVP_TIMER_SPAN);
        }
    }

    /// The `PLAYER_FLAGS` watch (`0x6e102c`): a change in `0x40300` re-stamps unless the timer flag
    /// was set before and after (the PvP cooldown's stamp, `0x6de750`).
    fn watch(&mut self, old: u32, new: u32, now: std::time::Instant) {
        if (old ^ new) & 0x40300 == 0 {
            return;
        }
        if new & PVP_DESIRED != 0 {
            self.stamp = None;
        } else if new & PVP_TIMER == 0 || old & PVP_TIMER == 0 {
            self.stamp = Some(now + PVP_TIMER_SPAN);
        }
    }

    /// The PvP toggle's timer (`0x6de7a0`).
    fn toggle_timer(&self, now: std::time::Instant) -> i32 {
        if self.flags.unwrap_or(0) & PVP_TIMER == 0 {
            return PVP_TIMER_SPAN.as_millis() as i32;
        }
        self.stamp
            .and_then(|t| t.checked_duration_since(now))
            .filter(|left| !left.is_zero())
            .map_or(-1, |left| left.as_millis() as i32)
    }
}

/// Track the toggle timer off our `PLAYER_FLAGS` and push it for `GetPVPTimer`.
fn feed_pvp_timer(
    script: Option<NonSendMut<UiScript>>,
    self_store: Query<&ObjectStore, With<SelfPlayer>>,
    mut entered: MessageReader<crate::net::WorldEnterCascadeMessage>,
    mut timer: Local<PvpTimer>,
) {
    if entered.read().next().is_some() {
        *timer = PvpTimer::default();
    }
    let Some(mut script) = script else {
        return;
    };
    let now = std::time::Instant::now();
    if let Some(flags) = self_store.iter().next().map(|s| s.0.player_flags()) {
        match timer.flags {
            None => timer.enter(flags, now),
            Some(old) => timer.watch(old, flags, now),
        }
        timer.flags = Some(flags);
    }
    script.set_pvp_toggle_timer(timer.toggle_timer(now));
}

/// The overhead name's medal strings ([`crate::nameplates::PvpNameStrings`]), read off a VM the
/// first frame it is in the world. Every VM is installed whole, outside `Update` (the boot VM with
/// `GlobalStrings.lua` run, a world VM after its FrameXML and addon load), so the read sees what
/// the load defined; any other frame pays one compare.
fn feed_pvp_name_strings(
    script: Option<NonSend<UiScript>>,
    mut strings: ResMut<crate::nameplates::PvpNameStrings>,
) {
    let Some(script) = script else {
        return;
    };
    if strings.session() != Some(script.session()) {
        strings.set(script.session(), pvp_name_strings(&script));
    }
}

/// The [`benilla_ui::script::pvp_name_global_keys`] that `script` defines, with their values.
fn pvp_name_strings(script: &UiScript) -> HashMap<String, String> {
    benilla_ui::script::pvp_name_global_keys()
        .into_iter()
        .filter_map(|key| benilla_ui::strings::global(script.lua(), &key).map(|value| (key, value)))
        .collect()
}

/// Read the honor block off our own descriptor, `None` until it streams. That decides only when
/// the first push and its events happen: the reference reads a field it was never sent as 0.
fn honor_snapshot(store: &ObjectStore) -> Option<HonorState> {
    let f = &store.0;
    // Presence is tested on a private field: the public rank byte streams with the unit block.
    let today_hk = f.player_kills_today()?;
    Some(HonorState {
        today_hk,
        today_honor: f.player_today_contribution().unwrap_or(0),
        yesterday_hk: f.player_kills_yesterday().unwrap_or(0),
        yesterday_honor: f.player_yesterday_contribution().unwrap_or(0),
        lifetime_hk: f.player_lifetime_honorable_kills().unwrap_or(0),
        // The highest lifetime rank, private `PLAYER_FIELD_BYTES` byte 3 (`0x611bc4`).
        highest_rank: f.player_honor_rank().unwrap_or(0),
        rank: f.player_pvp_rank().unwrap_or(0),
        honor_currency: f.player_honor_currency().unwrap_or(0),
        arena_currency: f.player_arena_currency().unwrap_or(0),
    })
}

/// What moved between two snapshots, by the watched fields only: `(kills, rank, currency)`. A
/// first push fires all three.
fn events_for(before: Option<&HonorState>, after: &HonorState) -> (bool, bool, bool) {
    let Some(b) = before else {
        return (true, true, true);
    };
    // The low half of `PLAYER_FIELD_KILLS` (a two-byte watch), today's honor, the lifetime kills.
    let kills = |h: &HonorState| (h.today_hk, h.today_honor, h.lifetime_hk);
    let currency = |h: &HonorState| (h.honor_currency, h.arena_currency);
    (
        kills(b) != kills(after),
        b.rank != after.rank,
        currency(b) != currency(after),
    )
}

/// Push the self snapshot and the inspect reply, fire what moved, and drain the pane's request.
fn feed_honor(
    script: Option<NonSendMut<UiScript>>,
    self_store: Query<&ObjectStore, With<SelfPlayer>>,
    inspect_target: Res<crate::ui_inspect::InspectTarget>,
    selection: Res<crate::target::Selection>,
    group: Res<crate::ui_party::GroupState>,
    mut inspect_honor: ResMut<InspectHonor>,
    mut state: ResMut<HonorFeedState>,
    commands: Res<NetCommands>,
) {
    let Some(mut script) = script else {
        return;
    };
    let memo = state.vm.get(&script);

    // --- the self snapshot -------------------------------------------------------------------
    if let Ok(store) = self_store.single() {
        if let Some(fresh) = honor_snapshot(store) {
            if memo.last.as_ref() != Some(&fresh) {
                let (kills, ranks, currency) = events_for(memo.last.as_ref(), &fresh);
                script.set_honor(Some(fresh));
                memo.last = Some(fresh);
                if kills {
                    script.fire_event("PLAYER_PVP_KILLS_CHANGED", vec![]);
                }
                if ranks {
                    script.fire_event("PLAYER_PVP_RANK_CHANGED", vec![]);
                }
                if currency {
                    script.fire_event("HONOR_CURRENCY_UPDATE", vec![]);
                    script.fire_event("CURRENCY_DISPLAY_UPDATE", vec![]);
                }
            }
        }
    }

    // --- the inspect reply ------------------------------------------------------------------- The
    // reference's latch clears only, from a `NotifyInspect` naming another player, and on
    // `ClearInspectPlayer`. Deviation: we clear it whenever the inspected token's guid moves,
    // because our window re-resolves its token each frame and both tabs read it, so the reference's
    // rule would let the two tabs show different players. `ClearInspectPlayer` drops the token,
    // which clears the slot here too.
    let inspected = inspect_target
        .token
        .as_deref()
        .and_then(|t| crate::ui_unit::player_token_guid(t, &selection, &group));
    if inspect_honor
        .0
        .as_ref()
        .is_some_and(|reply| Some(reply.player_guid) != inspected)
    {
        // Dropped from the resource, so `HasInspectHonorData` and this store agree.
        inspect_honor.0 = None;
    }
    let held = inspect_honor.0.as_ref().map(|r| r.player_guid);
    if memo.last_inspect != held {
        script.set_inspect_honor(inspect_honor.0.as_ref().map(|r| InspectHonorData {
            guid: r.player_guid,
            today_hk: r.today_hk,
            today_honor: r.today_honor,
            yesterday_hk: r.yesterday_hk,
            yesterday_honor: r.yesterday_honor,
            lifetime_hk: r.lifetime_hk,
            highest_rank: r.highest_rank,
        }));
        memo.last_inspect = held;
        // Fired on a clear too, so the pane stops showing the previous player's numbers.
        script.fire_event("INSPECT_HONOR_UPDATE", Vec::<ScriptValue>::new());
    }

    // --- the pane's request ------------------------------------------------------------------
    // `RequestInspectHonorData()` takes no argument: it asks about the inspected token's guid now,
    // so a re-target before the Honor tab opens asks about the new player.
    let requests = script.take_inspect_honor_requests();
    if requests > 0 {
        match inspected {
            Some(guid) => {
                debug!("honor: inspect-honor request -> {guid:#x}");
                let _ = commands
                    .0
                    .send(ClientCommand::InspectHonorStats { target: guid });
            }
            // Nothing to ask about; the pane asks again from its next `OnShow`.
            None => debug!("honor: inspect-honor request with no resolvable target — not sent"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> HonorState {
        HonorState {
            today_hk: 3,
            lifetime_hk: 900,
            rank: 8,
            highest_rank: 9,
            honor_currency: 1_000,
            ..Default::default()
        }
    }

    #[test]
    fn each_event_fires_only_for_its_own_fields() {
        let base = state();

        let mut killed = base;
        killed.today_hk += 1;
        assert_eq!(events_for(Some(&base), &killed), (true, false, false));
        let mut earned = base;
        earned.today_honor = 40;
        assert_eq!(events_for(Some(&base), &earned), (true, false, false));

        let mut ranked = base;
        ranked.rank = 9;
        assert_eq!(events_for(Some(&base), &ranked), (false, true, false));

        let mut paid = base;
        paid.arena_currency = 50;
        assert_eq!(events_for(Some(&base), &paid), (false, false, true));

        // Unwatched in the reference, so these fire nothing.
        for mutate in [
            (|h: &mut HonorState| h.yesterday_hk = 42) as fn(&mut HonorState),
            |h: &mut HonorState| h.yesterday_honor = 900,
            |h: &mut HonorState| h.highest_rank = 18,
        ] {
            let mut quiet = base;
            mutate(&mut quiet);
            assert_eq!(
                events_for(Some(&base), &quiet),
                (false, false, false),
                "an unwatched field must fire nothing"
            );
        }
    }

    /// Otherwise a pane whose numbers arrived before it existed would never paint.
    #[test]
    fn the_pvp_timer_starts_when_the_flag_comes_on_and_runs_out_to_minus_one() {
        let t0 = std::time::Instant::now();
        let mut t = PvpTimer::default();
        t.enter(PVP_DESIRED, t0);
        t.flags = Some(PVP_DESIRED);
        assert_eq!(t.toggle_timer(t0), 300_000, "no timer flag: the full span");
        // Turned off: the server sets the timer flag, and the watch stamps five minutes.
        t.watch(PVP_DESIRED, PVP_TIMER, t0);
        t.flags = Some(PVP_TIMER);
        let left = t.toggle_timer(t0 + std::time::Duration::from_secs(60));
        assert!((239_000..=240_000).contains(&left), "{left}");
        assert_eq!(t.toggle_timer(t0 + PVP_TIMER_SPAN), -1, "past the stamp");
        // An unrelated flag change with the timer kept does not re-stamp.
        t.watch(PVP_TIMER, PVP_TIMER | 0x1, t0 + PVP_TIMER_SPAN);
        assert_eq!(t.toggle_timer(t0 + PVP_TIMER_SPAN), -1);
    }

    /// A login with the timer flag already on keeps the zeroed stamp, so the timer reads -1.
    #[test]
    fn a_login_inside_the_timer_reads_it_as_run_out() {
        let now = std::time::Instant::now();
        let mut t = PvpTimer::default();
        t.enter(PVP_TIMER, now);
        t.flags = Some(PVP_TIMER);
        assert_eq!(t.toggle_timer(now), -1);
    }

    #[test]
    fn the_first_push_fires_all_three() {
        assert_eq!(events_for(None, &state()), (true, true, true));
    }

    fn vm(src: &str) -> UiScript {
        let script = UiScript::new().expect("a VM");
        script.run(src).expect("globals set");
        script
    }

    /// The read takes the overhead name's medal keys a VM defines, and nothing else.
    #[test]
    fn the_name_strings_read_takes_only_the_medal_keys() {
        let strings = pvp_name_strings(&vm(r#"PVP_MEDAL1 = "Guardian"
               UNIT_PVP_NAME = "%s %s"
               GARBAGE_KEY = "x""#));
        assert_eq!(
            strings.get("PVP_MEDAL1").map(String::as_str),
            Some("Guardian")
        );
        assert!(
            !strings.contains_key("UNIT_PVP_NAME"),
            "only the medal keys"
        );
        assert!(!strings.contains_key("GARBAGE_KEY"));
    }

    /// Each VM is read once, the first frame it is in the world, so a world VM whose addon renames
    /// a title reaches the overhead line.
    #[test]
    fn each_new_vm_is_read_once() {
        use bevy::ecs::system::RunSystemOnce;
        let mut world = World::new();
        world.init_resource::<crate::nameplates::PvpNameStrings>();
        world.run_system_once(feed_pvp_name_strings).unwrap();
        assert_eq!(
            world
                .resource::<crate::nameplates::PvpNameStrings>()
                .session(),
            None,
            "no VM, no read"
        );
        let stock = r#"PVP_MEDAL1 = "Guardian""#;
        let boot = vm(stock);
        let boot_session = boot.session();
        world.insert_non_send_resource(boot);
        world.run_system_once(feed_pvp_name_strings).unwrap();
        let held = world.resource::<crate::nameplates::PvpNameStrings>();
        assert_eq!(held.session(), Some(boot_session));
        assert_eq!(held.get("PVP_MEDAL1"), Some("Guardian"));

        // The same VM is not read again: a global it sets later stays out.
        world
            .non_send_resource::<UiScript>()
            .run(r#"PVP_MEDAL1 = "Later""#)
            .expect("global set");
        world.run_system_once(feed_pvp_name_strings).unwrap();
        let held = world.resource::<crate::nameplates::PvpNameStrings>();
        assert_eq!(held.get("PVP_MEDAL1"), Some("Guardian"));

        let world_vm = vm(&format!("{stock}\nPVP_MEDAL1 = \"Hüter\""));
        let world_session = world_vm.session();
        world.insert_non_send_resource(world_vm);
        world.run_system_once(feed_pvp_name_strings).unwrap();
        let held = world.resource::<crate::nameplates::PvpNameStrings>();
        assert_eq!(held.session(), Some(world_session));
        assert_eq!(held.get("PVP_MEDAL1"), Some("Hüter"));
    }
}
