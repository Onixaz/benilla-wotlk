//! Honor: our own honor fields as the PvP window's honor pane reads them, the PvP toggle timer,
//! and the inspect-honor reply. The honor block streams only to its owner, so another player's
//! numbers come only from `MSG_INSPECT_HONOR_STATS`; the stock pane re-asks from its `OnShow`
//! while it holds nothing, the only retry.
//!
//! The verbs read our descriptor when called. The pane's events fire from our values handler's
//! field watches (`0x6e5180`'s list), in field order: `PLAYER_BYTES_3` fires
//! `PLAYER_PVP_RANK_CHANGED` (`0x6e5671`), today's kills (the low half), today's honor and the
//! lifetime kills fire `PLAYER_PVP_KILLS_CHANGED` (`0x6e55fc`, `0x6e564b`, `0x6e5622`), each with
//! `"player"` (`0x6cddd6`), and either currency fires `HONOR_CURRENCY_UPDATE` and
//! `CURRENCY_DISPLAY_UPDATE` (`0x6da3c0`, `0x6da430`). Yesterday's numbers have no watch.
//!
//! Deviation: a currency loss's sound kit (`0x6da400`) is not played, as no sound kit is keyed by
//! name here.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use bevy::prelude::*;

use benilla_protocol::ObjectFields;
use benilla_ui::script::{HonorState, ScriptValue, UiScript};

use crate::net::{ClientCommand, NetCommands};
use crate::ui_script::live::UiEvents;

/// The inspect-honor reply's handler; the honor award, `SMSG_PVP_CREDIT`, is the chat family's.
mod net {
    use benilla_protocol::{SessionEvent, SessionEventKind};
    use benilla_ui::script::InspectHonorData;
    use bevy::prelude::*;

    use crate::net::NetHandlerApp;
    use crate::ui_script::live::UiEvents;

    pub(super) fn register(app: &mut App) {
        app.net_handler(SessionEventKind::InspectHonorStats, on_inspect_stats);
    }

    /// `MSG_INSPECT_HONOR_STATS` (`0x5e7a50`): the target's reply alone (`0x5e7a68`), held, then `INSPECT_HONOR_UPDATE` (`0x5e7aee`).
    pub(super) fn on_inspect_stats(
        In(ev): In<SessionEvent>,
        target: Res<crate::ui_inspect::InspectGuid>,
        mut ui: ResMut<UiEvents>,
    ) {
        let SessionEvent::InspectHonorStats(r) = ev else {
            return;
        };
        if r.player_guid == 0 || r.player_guid != target.get() {
            return;
        }
        let data = InspectHonorData {
            guid: r.player_guid,
            today_hk: r.today_hk,
            today_honor: r.today_honor,
            yesterday_hk: r.yesterday_hk,
            yesterday_honor: r.yesterday_honor,
            lifetime_hk: r.lifetime_hk,
            highest_rank: r.highest_rank,
        };
        ui.write(move |s| s.set_inspect_honor(Some(data)));
        ui.fire("INSPECT_HONOR_UPDATE", Vec::new());
    }
}

pub(crate) struct UiHonorPlugin;

impl Plugin for UiHonorPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<PvpCooldown>()
            .init_resource::<crate::ui_inspect::InspectGuid>()
            .init_resource::<crate::nameplates::PvpNameStrings>()
            .add_systems(
                Update,
                drain_inspect_honor_requests.in_set(crate::ui_script::UiDrain),
            )
            // A VM holder, so after the tick; before the driver, so a new VM's strings reach the
            // lines the frame it arrives.
            .add_systems(
                Update,
                feed_pvp_name_strings
                    .in_set(crate::ui_script::UiDrain)
                    .before(crate::nameplates::drive_nameplates),
            );
    }
}

/// `RequestInspectHonorData()` (`0x5e9140`): a send for the inspect target (`0x5e91a1`), none without one (`0x5e916c`).
fn drain_inspect_honor_requests(
    (script, _live): (Option<NonSendMut<UiScript>>, crate::ui_script::LendLive),
    target: Res<crate::ui_inspect::InspectGuid>,
    commands: Res<NetCommands>,
) {
    let Some(mut script) = script else {
        return;
    };
    for _ in 0..script.take_inspect_honor_requests() {
        let target = target.get();
        if target == 0 {
            debug!("honor: inspect-honor request with no inspect target — not sent");
            continue;
        }
        let _ = commands.0.send(ClientCommand::InspectHonorStats { target });
    }
}

/// `PLAYER_FLAGS_PVP_DESIRED` and `PLAYER_FLAGS_PVP_TIMER`.
const PVP_DESIRED: u32 = 0x200;
const PVP_TIMER: u32 = benilla_ui::script::PLAYER_FLAGS_PVP_TIMER;

/// The PvP toggle timer's five minutes (`0x6de789`).
const PVP_TIMER_SPAN: Duration = Duration::from_millis(300_000);

/// The active player's PvP cooldown stamp (`[player+0x1940]`), `None` for the zero stamp.
#[derive(Resource, Default)]
pub(crate) struct PvpCooldown {
    stamp: Option<Instant>,
}

impl PvpCooldown {
    /// Our create's init (`0x6e81f2`): desired or a running timer keeps the stamp zero, else it starts.
    pub(crate) fn enter(&mut self, flags: u32, now: Instant) {
        self.stamp = None;
        if flags & PVP_DESIRED == 0 && flags & PVP_TIMER == 0 {
            self.stamp = Some(now + PVP_TIMER_SPAN);
        }
    }

    /// The `PLAYER_FLAGS` watch (`0x6e102c`): a change in `0x40300` re-stamps (`0x6de750`).
    pub(crate) fn watch(&mut self, old: u32, new: u32, now: Instant) {
        if (old ^ new) & 0x40300 == 0 {
            return;
        }
        if new & PVP_DESIRED != 0 {
            self.stamp = None;
        } else if new & PVP_TIMER == 0 || old & PVP_TIMER == 0 {
            self.stamp = Some(now + PVP_TIMER_SPAN);
        }
    }

    /// The PvP toggle timer (`0x6de7a0`): the full span without the flag, else ms left, -1 once passed.
    pub(crate) fn toggle_timer(&self, flags: u32, now: Instant) -> i32 {
        if flags & PVP_TIMER == 0 {
            return PVP_TIMER_SPAN.as_millis() as i32;
        }
        self.stamp
            .and_then(|t| t.checked_duration_since(now))
            .filter(|left| !left.is_zero())
            .map_or(-1, |left| left.as_millis() as i32)
    }
}

/// What the honor verbs read live beside our descriptor: the cooldown stamp.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct HonorView<'w> {
    cooldown: Option<Res<'w, PvpCooldown>>,
}

impl HonorView<'_> {
    /// `GetPVPTimer`'s timer over our descriptor.
    pub(crate) fn toggle_timer(&self, fields: &ObjectFields) -> i32 {
        let flags = fields.player_flags();
        match self.cooldown.as_deref() {
            Some(c) => c.toggle_timer(flags, Instant::now()),
            None => PvpCooldown::default().toggle_timer(flags, Instant::now()),
        }
    }
}

/// Our own honor fields' edges, `(index, old, new)` in field order, as their watches fire.
#[cfg(test)]
fn own_fields_changed(edges: &[(u16, u32, u32)], ui: &mut UiEvents) {
    for &(index, old, new) in edges {
        own_field_changed(index, old, new, ui);
    }
}

/// One of our own honor fields' edges, as its watch fires.
pub(crate) fn own_field_changed(index: u16, old: u32, new: u32, ui: &mut UiEvents) {
    use benilla_protocol::field::{
        FIELD_PLAYER_BYTES_3, FIELD_PLAYER_FIELD_ARENA_CURRENCY, FIELD_PLAYER_FIELD_HONOR_CURRENCY,
        FIELD_PLAYER_FIELD_KILLS, FIELD_PLAYER_FIELD_LIFETIME_HONORABLE_KILLS,
        FIELD_PLAYER_FIELD_TODAY_CONTRIBUTION,
    };
    let player = || vec![ScriptValue::Str("player".into())];
    {
        match index {
            FIELD_PLAYER_BYTES_3 => ui.fire("PLAYER_PVP_RANK_CHANGED", player()),
            // A two-byte watch: today's kills, not yesterday's high half.
            FIELD_PLAYER_FIELD_KILLS if (old ^ new) & 0xffff != 0 => {
                ui.fire("PLAYER_PVP_KILLS_CHANGED", player());
            }
            FIELD_PLAYER_FIELD_TODAY_CONTRIBUTION | FIELD_PLAYER_FIELD_LIFETIME_HONORABLE_KILLS => {
                ui.fire("PLAYER_PVP_KILLS_CHANGED", player());
            }
            FIELD_PLAYER_FIELD_HONOR_CURRENCY | FIELD_PLAYER_FIELD_ARENA_CURRENCY => {
                ui.fire("HONOR_CURRENCY_UPDATE", Vec::new());
                ui.fire("CURRENCY_DISPLAY_UPDATE", Vec::new());
            }
            _ => {}
        }
    }
}

/// Whether `index` is one of our honor watches.
pub(crate) fn is_honor_watch(index: u16) -> bool {
    use benilla_protocol::field::{
        FIELD_PLAYER_BYTES_3, FIELD_PLAYER_FIELD_ARENA_CURRENCY, FIELD_PLAYER_FIELD_HONOR_CURRENCY,
        FIELD_PLAYER_FIELD_KILLS, FIELD_PLAYER_FIELD_LIFETIME_HONORABLE_KILLS,
        FIELD_PLAYER_FIELD_TODAY_CONTRIBUTION,
    };
    matches!(
        index,
        FIELD_PLAYER_BYTES_3
            | FIELD_PLAYER_FIELD_KILLS
            | FIELD_PLAYER_FIELD_TODAY_CONTRIBUTION
            | FIELD_PLAYER_FIELD_LIFETIME_HONORABLE_KILLS
            | FIELD_PLAYER_FIELD_HONOR_CURRENCY
            | FIELD_PLAYER_FIELD_ARENA_CURRENCY
    )
}

/// The overhead name's medal and flag-tag strings ([`crate::nameplates::PvpNameStrings`]), read
/// off a VM the first frame it is in the world. Every VM is installed whole, outside `Update` (the
/// boot VM with `GlobalStrings.lua` run, a world VM after its FrameXML and addon load), so the read
/// sees what the load defined; any other frame pays one compare.
fn feed_pvp_name_strings(
    (script, _live): (Option<NonSend<UiScript>>, crate::ui_script::LendLive),
    mut strings: ResMut<crate::nameplates::PvpNameStrings>,
) {
    let Some(script) = script else {
        return;
    };
    if strings.session() != Some(script.session()) {
        strings.set(script.session(), pvp_name_strings(&script));
    }
}

/// The [`benilla_ui::script::pvp_name_global_keys`] and the name flag tags
/// ([`crate::nameplates::FLAG_TAG_KEYS`]) that `script` defines, with their values.
fn pvp_name_strings(script: &UiScript) -> HashMap<String, String> {
    benilla_ui::script::pvp_name_global_keys()
        .into_iter()
        .chain(
            crate::nameplates::FLAG_TAG_KEYS
                .iter()
                .map(|(_, key)| (*key).to_owned()),
        )
        .filter_map(|key| benilla_ui::strings::global(script.lua(), &key).map(|value| (key, value)))
        .collect()
}

/// Our honor block off our descriptor, `None` until its private fields stream.
pub(crate) fn honor_of(f: &ObjectFields) -> Option<HonorState> {
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
        // No current rank streams: 3.3.5a's `PLAYER_BYTES_3` byte 3 is the arena faction.
        rank: 0,
        honor_currency: f.player_honor_currency().unwrap_or(0),
        arena_currency: f.player_arena_currency().unwrap_or(0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each watch fires its own event in field order; the unwatched fields fire nothing.
    #[test]
    fn each_watch_fires_its_own_event_in_field_order() {
        use benilla_protocol::field::{
            FIELD_PLAYER_BYTES_3, FIELD_PLAYER_FIELD_ARENA_CURRENCY, FIELD_PLAYER_FIELD_KILLS,
            FIELD_PLAYER_FIELD_LIFETIME_HONORABLE_KILLS, FIELD_PLAYER_FIELD_YESTERDAY_CONTRIBUTION,
        };
        let mut ui = UiEvents::default();
        own_fields_changed(
            &[
                (FIELD_PLAYER_BYTES_3, 0, 1 << 24),
                (FIELD_PLAYER_FIELD_KILLS, 3, 3 | (5 << 16)),
                (FIELD_PLAYER_FIELD_KILLS, 3, 4),
                (FIELD_PLAYER_FIELD_LIFETIME_HONORABLE_KILLS, 900, 901),
                (FIELD_PLAYER_FIELD_YESTERDAY_CONTRIBUTION, 0, 40),
                (FIELD_PLAYER_FIELD_ARENA_CURRENCY, 0, 50),
            ],
            &mut ui,
        );
        assert_eq!(
            ui.names(),
            [
                "PLAYER_PVP_RANK_CHANGED",
                "PLAYER_PVP_KILLS_CHANGED",
                "PLAYER_PVP_KILLS_CHANGED",
                "HONOR_CURRENCY_UPDATE",
                "CURRENCY_DISPLAY_UPDATE",
            ]
        );
    }

    #[test]
    fn the_pvp_timer_starts_when_the_flag_comes_on_and_runs_out_to_minus_one() {
        let t0 = Instant::now();
        let mut t = PvpCooldown::default();
        t.enter(PVP_DESIRED, t0);
        assert_eq!(
            t.toggle_timer(PVP_DESIRED, t0),
            300_000,
            "no timer flag: the full span"
        );
        // Turned off: the server sets the timer flag, and the watch stamps five minutes.
        t.watch(PVP_DESIRED, PVP_TIMER, t0);
        let left = t.toggle_timer(PVP_TIMER, t0 + Duration::from_secs(60));
        assert!((239_000..=240_000).contains(&left), "{left}");
        assert_eq!(
            t.toggle_timer(PVP_TIMER, t0 + PVP_TIMER_SPAN),
            -1,
            "past the stamp"
        );
        // An unrelated flag change with the timer kept does not re-stamp.
        t.watch(PVP_TIMER, PVP_TIMER | 0x1, t0 + PVP_TIMER_SPAN);
        assert_eq!(t.toggle_timer(PVP_TIMER, t0 + PVP_TIMER_SPAN), -1);
    }

    /// A login with the timer flag on keeps the zeroed stamp (-1); a fresh create drops the old stamp.
    #[test]
    fn a_login_inside_the_timer_reads_it_as_run_out() {
        let now = Instant::now();
        let mut t = PvpCooldown::default();
        t.enter(0, now);
        assert!(t.stamp.is_some());
        t.enter(PVP_TIMER, now);
        assert_eq!(t.toggle_timer(PVP_TIMER, now), -1);
    }

    fn vm(src: &str) -> UiScript {
        let script = UiScript::new().expect("a VM");
        script.run(src).expect("globals set");
        script
    }

    /// The read takes the overhead name's medal and flag-tag keys a VM defines, and nothing else.
    #[test]
    fn the_name_strings_read_takes_only_the_medal_and_tag_keys() {
        let strings = pvp_name_strings(&vm(r#"PVP_MEDAL1 = "Guardian"
               CHAT_FLAG_DND = "<DND>"
               UNIT_PVP_NAME = "%s %s"
               GARBAGE_KEY = "x""#));
        assert_eq!(
            strings.get("PVP_MEDAL1").map(String::as_str),
            Some("Guardian")
        );
        assert_eq!(
            strings.get("CHAT_FLAG_DND").map(String::as_str),
            Some("<DND>")
        );
        assert!(!strings.contains_key("CHAT_FLAG_AFK"), "an undefined tag");
        assert!(
            !strings.contains_key("UNIT_PVP_NAME"),
            "only the medal and tag keys"
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
