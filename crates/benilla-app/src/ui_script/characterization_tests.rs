//! Characterization: whole headless frames with the production in-game UI loaded through the real
//! world-entry path, scripted packets sent through `apply_net_updates`, and a Lua observer addon
//! recording every event its frame sees with the verb answers a handler would read at that moment.
//! The pins are today's results; a change to one cites the 12340 behaviour it moves toward.

use std::collections::BTreeMap;

use benilla_protocol::field::{FIELD_UNIT_HEALTH, FIELD_UNIT_LEVEL, FIELD_UNIT_MAXHEALTH};
use benilla_protocol::messages::ObjectType;
use benilla_protocol::{EntityKind, ObjectFields, SessionEvent};
use bevy::prelude::*;
use crossbeam_channel::Sender;

use crate::local_state::test_env::{EnvGuard, ENV_LOCK};
use crate::net::NetEvents;

/// Our character's guid.
pub(super) const ME: u64 = 0x10;

const OBSERVER_TOC: &str = "## Interface: 30300\n## Title: CharzObserver\nObserver.lua\n";

/// The observer: every event, its arguments, and each probe verb's answer at that moment.
const OBSERVER_LUA: &str = r##"
CHARZ_LOG = {}
local function popup()
    for i = 1, 4 do
        local p = _G["StaticPopup" .. i]
        if p and p:IsShown() and p.which == "DEATH" then
            local t = _G["StaticPopup" .. i .. "Text"]
            return t and t:GetText()
        end
    end
end
local function button2()
    for i = 1, 4 do
        local p = _G["StaticPopup" .. i]
        if p and p:IsShown() and p.which == "DEATH" then
            local b = _G["StaticPopup" .. i .. "Button2"]
            return b:IsShown() and b:GetText()
        end
    end
end
local VERBS = {
    { "taxi", function() return UnitOnTaxi("player") end },
    { "control", function() return HasFullControl() end },
    { "dead", function() return UnitIsDeadOrGhost("player") end },
    { "isdead", function() return UnitIsDead("player") end },
    { "ghost", function() return UnitIsGhost("player") end },
    { "arena", function() return IsActiveBattlefieldArena() end },
    { "mounted", function() return IsMounted() end },
    { "combat", function() return UnitAffectingCombat("player") end },
    { "lockdown", function() return InCombatLockdown() end },
    { "zone", function() return GetZoneText() end },
    { "quests", function() return GetNumQuestLogEntries() end },
    { "pet", function() return UnitExists("pet") end },
    { "petui", function() return HasPetUI() end },
    { "mounts", function() return GetNumCompanions("MOUNT") end },
    { "popup", popup },
    { "button2", button2 },
    { "soulstone", function() return HasSoulstone() end },
    { "special", function() return GetQuestLogSpecialItemInfo(2) ~= nil end },
    { "watchitem", function() return WatchFrameItem1 ~= nil and WatchFrameItem1:IsShown() end },
    { "release", function() return GetReleaseTimeRemaining() end },
    { "reclaim", function() return GetCorpseRecoveryDelay() end },
    { "summon", function() return GetSummonConfirmTimeLeft() end },
    { "summoner", function() return GetSummonConfirmSummoner() end },
    { "falling", function() return IsFalling() end },
    { "nores", function() return CannotBeResurrected() end },
    { "healer", function() return CheckSpiritHealerDist() end },
    { "restimer", function() return ResurrectHasTimer() end },
    { "reset", function() return GetQuestResetTime() end },
}
local function answers()
    local verbs = {}
    for _, v in ipairs(VERBS) do
        local ok, a = pcall(v[2])
        verbs[#verbs + 1] = v[1] .. "=" .. (ok and tostring(a) or "error")
    end
    return table.concat(verbs, ";")
end
CHARZ_ANSWERS = answers
local f = CreateFrame("Frame")
f:RegisterAllEvents()
f:SetScript("OnEvent", function(_, e, ...)
    local args = {}
    for i = 1, select("#", ...) do args[#args + 1] = tostring((select(i, ...))) end
    CHARZ_LOG[#CHARZ_LOG + 1] = e .. "\t" .. table.concat(args, " ") .. "\t" .. answers()
end)
"##;

/// One event as the observer saw it.
#[derive(Debug, Clone)]
pub(super) struct Seen {
    pub event: String,
    pub args: String,
    pub verbs: BTreeMap<String, String>,
}

impl Seen {
    fn parse(line: &str) -> Self {
        let mut parts = line.splitn(3, '\t');
        let event = parts.next().unwrap_or_default().to_string();
        let args = parts.next().unwrap_or_default().to_string();
        let verbs = parts
            .next()
            .unwrap_or_default()
            .split(';')
            .filter_map(|kv| kv.split_once('='))
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        Self { event, args, verbs }
    }

    /// `EVENT args | k=v ...` over `verbs`, in the order given.
    pub fn line(&self, verbs: &[&str]) -> String {
        let mut s = self.event.clone();
        if !self.args.is_empty() {
            s.push(' ');
            s.push_str(&self.args);
        }
        if !verbs.is_empty() {
            s.push_str(" |");
            for v in verbs {
                let a = self.verbs.get(*v).map_or("?", String::as_str);
                s.push_str(&format!(" {v}={a}"));
            }
        }
        s
    }
}

/// The lines of `seen` whose event is in `events`, with the `verbs` answers.
pub(super) fn pin(seen: &[Seen], events: &[&str], verbs: &[&str]) -> Vec<String> {
    seen.iter()
        .filter(|s| events.contains(&s.event.as_str()))
        .map(|s| s.line(verbs))
        .collect()
}

/// Set in the child process that runs one rig test; the parent only launches it.
const CHILD: &str = "BENILLA_CHARZ_CHILD";

/// One child at a time: each opens the install's tables.
static CHILD_RUNS: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Runs the calling test alone in a child copy of this test binary, so the rig's process-wide
/// state (environment, statics, parked materials) never reaches the tests running beside it.
fn run_in_child() {
    let name = std::thread::current()
        .name()
        .expect("a test runs on a thread named after it")
        .to_string();
    let _one = CHILD_RUNS.lock().unwrap_or_else(|e| e.into_inner());
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args([&name, "--exact", "--include-ignored", "--nocapture"])
        .env(CHILD, "1")
        .output()
        .unwrap();
    let report = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.status.success() && report.contains("1 passed"),
        "the child run of {name}:\n{report}"
    );
}

/// The headless client logged in with the production UI and the observer, and the packet line.
pub(super) struct Rig {
    pub app: App,
    tx: Sender<SessionEvent>,
    seen: Vec<Seen>,
    _env: (EnvGuard, EnvGuard),
    tmp: std::path::PathBuf,
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl Drop for Rig {
    fn drop(&mut self) {
        // The app first: it holds files under the scratch home open.
        drop(std::mem::replace(&mut self.app, App::empty()));
        let _ = std::fs::remove_dir_all(&self.tmp);
    }
}

impl Rig {
    /// A client in the world: `Connected`, the account data times, our create, and frames until the
    /// in-game UI has loaded and `PLAYER_LOGIN` has fired. `None` (a skip) with no install.
    pub fn login(fields: &[(u16, u32)]) -> Option<Self> {
        Self::login_after(Vec::new(), fields)
    }

    /// [`Self::login`] with `before` on the wire ahead of our create, in the same frame.
    pub fn login_after(before: Vec<SessionEvent>, fields: &[(u16, u32)]) -> Option<Self> {
        benilla_formats::wow_data_or_skip!(None);
        if std::env::var_os(CHILD).is_none() {
            run_in_child();
            return None;
        }
        let lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = std::env::temp_dir().join(format!(
            "benilla-charz-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        let home = tmp.join("benilla-config");
        let dir = home.join("Interface/AddOns/CharzObserver");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("CharzObserver.toc"), OBSERVER_TOC).unwrap();
        std::fs::write(dir.join("Observer.lua"), OBSERVER_LUA).unwrap();
        let env = (
            EnvGuard::unset("WOW_CAPTURE"),
            EnvGuard::set("BENILLA_HOME", home.to_str().unwrap()),
        );

        let mut app = crate::game_plugins::schedule_tests::headless_client();
        // A stub device, so every main-world system that makes GPU resources (the world store's light
        // buffer, the model and portrait targets) runs, and the world store opens the install's
        // tables; nothing renders. A system whose parameter is still missing panics the frame.
        let (device, queue) = wgpu::Device::noop(&wgpu::DeviceDescriptor::default());
        app.insert_resource(bevy::render::renderer::RenderDevice::from(device))
            .insert_resource(bevy::render::renderer::RenderQueue(std::sync::Arc::new(
                bevy::render::renderer::WgpuWrapper::new(queue),
            )));
        // The primary window winit would make: the player controller, which applies the server's
        // control and movement edges, takes it as a `Single`.
        app.world_mut().spawn((
            Window::default(),
            bevy::window::CursorOptions::default(),
            bevy::window::PrimaryWindow,
        ));
        // What `register_asset_loaders` adds in the real app; nothing loads into them here.
        app.init_asset::<benilla_assets::M2Model>()
            .init_asset::<benilla_assets::WmoModel>()
            .init_asset::<benilla_assets::AdtTile>()
            .init_asset::<benilla_assets::WdtIndex>();
        let (tx, rx) = crossbeam_channel::unbounded();
        app.insert_resource(NetEvents(rx));
        let mut rig = Self {
            app,
            tx,
            seen: Vec::new(),
            _env: env,
            tmp,
            _lock: lock,
        };
        rig.send(SessionEvent::Connected {
            self_guid: ME,
            name: "Probesix".into(),
            billing_time_rested: 0,
            expansion: 2,
            tutorial_flags: None,
            addon_info: Some(Vec::new()),
        });
        rig.send(SessionEvent::AccountDataTimes(
            benilla_protocol::messages::AccountDataTimes {
                server_time: 1,
                flag: 1,
                mask: 0xff,
                times: [Some(0); 8],
            },
        ));
        for ev in before {
            rig.send(ev);
        }
        rig.send(self_create(fields));
        for _ in 0..30 {
            rig.run(1);
            if rig.seen.iter().any(|s| s.event == "PLAYER_LOGIN") {
                break;
            }
        }
        // The frames after the load: the first feed push and the entering-world burst.
        rig.run(3);
        Some(rig)
    }

    pub fn send(&self, ev: SessionEvent) {
        self.tx.send(ev).unwrap();
    }

    /// `n` whole frames, the observer drained after each.
    pub fn run(&mut self, n: usize) {
        for _ in 0..n {
            self.app.update();
            let lines = self
                .app
                .world()
                .get_non_send_resource::<benilla_ui::script::UiScript>()
                .and_then(|s| {
                    s.eval::<Vec<String>>("local t = CHARZ_LOG or {}; CHARZ_LOG = {}; return t")
                        .ok()
                })
                .unwrap_or_default();
            self.seen.extend(lines.iter().map(|l| Seen::parse(l)));
        }
    }

    /// The probe verbs' answers now, outside any event, as `Seen` with the event `NOW`.
    pub fn now(&self) -> Seen {
        let answers: String = self
            .app
            .world()
            .non_send_resource::<benilla_ui::script::UiScript>()
            .eval("return CHARZ_ANSWERS()")
            .expect("the observer is loaded");
        Seen::parse(&format!("NOW		{answers}"))
    }

    /// The probe verbs' answers with the `World` lent, as a script call between frames reads them.
    pub fn live_now(&mut self) -> Seen {
        let answers = super::live::with_live_world(self.app.world_mut(), |s| {
            s.eval::<String>("return CHARZ_ANSWERS()")
        })
        .expect("a VM")
        .expect("the observer is loaded");
        Seen::parse(&format!("NOW		{answers}"))
    }

    /// Everything seen since the last take.
    pub fn take(&mut self) -> Vec<Seen> {
        std::mem::take(&mut self.seen)
    }

    /// Send `events` as one frame's packets and run `frames` frames; what the observer saw.
    pub fn packets(&mut self, events: Vec<SessionEvent>, frames: usize) -> Vec<Seen> {
        self.take();
        for ev in events {
            self.send(ev);
        }
        self.run(frames);
        self.take()
    }
}

/// `OBJECT_FIELD_TYPE` as a server sends it for a player (object, unit, player), which the live
/// view's unit lookup tests; a scenario whose verbs read the view sends it.
pub(super) const OBJECT_TYPE_PLAYER: (u16, u32) = (2, 0x19);

/// Our own create block, with `extra` fields over a live level-80 body.
pub(super) fn self_create(extra: &[(u16, u32)]) -> SessionEvent {
    let mut pairs = vec![
        (FIELD_UNIT_HEALTH, 1000),
        (FIELD_UNIT_MAXHEALTH, 1000),
        (FIELD_UNIT_LEVEL, 80),
    ];
    pairs.extend_from_slice(extra);
    SessionEvent::ObjectCreate {
        guid: ME,
        kind: EntityKind::Player,
        display_id: None,
        position: [-8949.95, -132.493, 83.5312],
        orientation: 0.0,
        scale: 1.0,
        speeds: None,
        mover: None,
        transport_progress: None,
        transport: None,
        spline: None,
        vehicle: None,
        attack_target: None,
        spawned: false,
        fields: ObjectFields::from_pairs(&pairs).into_created(ObjectType::Player),
    }
}

/// Our own values delta.
pub(super) fn self_values(pairs: &[(u16, u32)]) -> SessionEvent {
    SessionEvent::ObjectValues {
        guid: ME,
        fields: ObjectFields::from_pairs(pairs),
    }
}

// ── Scenarios: today's results, pinned ───────────────────────────────────────────────────────────

use benilla_protocol::field::{
    FIELD_PLAYER_FLAGS, FIELD_PLAYER_INV_SLOT_HEAD, FIELD_PLAYER_QUEST_LOG_1_1, FIELD_UNIT_FLAGS,
    FIELD_UNIT_MOUNTDISPLAYID, FIELD_UNIT_SUMMON,
};

/// The pet's guid.
const PET: u64 = 0xF140_0000_0100_0001;
/// The quest the quest scenarios accept.
const QUEST: u32 = 7;
/// `PLAYER_FLAGS_GHOST`.
const GHOST: u32 = 0x10;
use benilla_protocol::field::FIELD_PLAYER_SELF_RES_SPELL;

fn flags(f: u32) -> SessionEvent {
    self_values(&[(FIELD_UNIT_FLAGS, f)])
}

fn control(allow_move: bool) -> SessionEvent {
    SessionEvent::ClientControl {
        mover: ME,
        allow_move,
    }
}

fn quest_template(src_item_id: u32) -> SessionEvent {
    SessionEvent::QuestTemplate(Box::new(benilla_protocol::messages::QuestTemplate {
        quest_id: QUEST,
        level: 80,
        zone_or_sort: 12,
        title: "Charz".into(),
        src_item_id,
        ..Default::default()
    }))
}

#[test]
fn login_fires_the_load_events_in_order() {
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    let seen = rig.take();
    assert_eq!(
        pin(
            &seen,
            &[
                "VARIABLES_LOADED",
                "PLAYER_LOGIN",
                "PLAYER_ENTERING_WORLD",
                "ADDON_LOADED"
            ],
            &["control", "dead"]
        ),
        vec![
            "ADDON_LOADED CharzObserver | control=1 dead=nil",
            "VARIABLES_LOADED | control=1 dead=nil",
            "ADDON_LOADED Blizzard_CombatLog | control=1 dead=nil",
            "PLAYER_LOGIN | control=1 dead=nil",
            "ADDON_LOADED Blizzard_TimeManager | control=1 dead=nil",
            "ADDON_LOADED Blizzard_CombatText | control=1 dead=nil",
            "PLAYER_ENTERING_WORLD | control=1 dead=nil",
            "ADDON_LOADED Blizzard_DebugTools | control=1 dead=nil",
        ]
    );
}

#[test]
fn reload_fires_the_load_events_again() {
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    rig.take();
    rig.app
        .world()
        .non_send_resource::<benilla_ui::script::UiScript>()
        .run("ReloadUI()")
        .unwrap();
    rig.run(4);
    let seen = rig.take();
    assert_eq!(
        pin(
            &seen,
            &[
                "VARIABLES_LOADED",
                "PLAYER_LOGIN",
                "PLAYER_ENTERING_WORLD",
                "ADDON_LOADED"
            ],
            &["control", "dead"]
        ),
        vec![
            "ADDON_LOADED CharzObserver | control=1 dead=nil",
            "VARIABLES_LOADED | control=1 dead=nil",
            "ADDON_LOADED Blizzard_CombatLog | control=1 dead=nil",
            "PLAYER_LOGIN | control=1 dead=nil",
            "ADDON_LOADED Blizzard_TimeManager | control=1 dead=nil",
            "ADDON_LOADED Blizzard_CombatText | control=1 dead=nil",
            "PLAYER_ENTERING_WORLD | control=1 dead=nil",
            "ADDON_LOADED Blizzard_DebugTools | control=1 dead=nil",
        ]
    );
}

/// `UnitIsDead` (`0x60f480`), `UnitIsDeadOrGhost` (`0x60f680`) and `UnitIsGhost` (`0x60f580`) read
/// the held unit at call time, so each death event reads the update its handler just applied.
#[test]
fn death_release_and_resurrection() {
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    let events = &[
        "PLAYER_DEAD",
        "PLAYER_ALIVE",
        "PLAYER_UNGHOST",
        "UNIT_HEALTH",
    ];
    let verbs = &["isdead", "dead", "ghost", "popup"];
    let died = rig.packets(vec![self_values(&[(FIELD_UNIT_HEALTH, 0)])], 3);
    let released = rig.packets(
        vec![self_values(&[
            (FIELD_UNIT_HEALTH, 1),
            (FIELD_PLAYER_FLAGS, GHOST),
        ])],
        3,
    );
    let revived = rig.packets(
        vec![self_values(&[
            (FIELD_UNIT_HEALTH, 1000),
            (FIELD_PLAYER_FLAGS, 0),
        ])],
        3,
    );
    assert_eq!(
        pin(&died, events, verbs),
        vec![
            "PLAYER_DEAD | isdead=1 dead=1 ghost=nil popup=You have died. Release to the nearest graveyard?",
            "UNIT_HEALTH player | isdead=1 dead=1 ghost=nil popup=You have died. Release to the nearest graveyard?",
        ]
    );
    assert_eq!(
        pin(&released, events, verbs),
        vec![
            "PLAYER_ALIVE | isdead=nil dead=1 ghost=1 popup=nil",
            "UNIT_HEALTH player | isdead=nil dead=1 ghost=1 popup=nil",
        ]
    );
    assert_eq!(
        pin(&revived, events, verbs),
        vec![
            "PLAYER_UNGHOST | isdead=nil dead=nil ghost=nil popup=nil",
            "UNIT_HEALTH player | isdead=nil dead=nil ghost=nil popup=nil",
        ]
    );
}

/// A death and the release in one frame: 12340 handles each update's health watcher in wire order
/// (`0x73f330`), so `PLAYER_DEAD` fires for the first with its release timer derived just before
/// it (`0x6dc0f0` -> `0x6dc070`, no timer bit: -1), then `PLAYER_ALIVE` for the second's health 1
/// (`0x73d5bc`), before the ghost bit's watcher asks for the corpse.
#[test]
fn death_and_release_in_one_frame_fire_in_wire_order() {
    let Some(mut rig) = Rig::login(&[]) else {
        return;
    };
    let seen = rig.packets(
        vec![
            self_values(&[(FIELD_UNIT_HEALTH, 0)]),
            self_values(&[(FIELD_UNIT_HEALTH, 1), (FIELD_PLAYER_FLAGS, GHOST)]),
        ],
        3,
    );
    assert_eq!(
        pin(
            &seen,
            &["PLAYER_DEAD", "PLAYER_ALIVE", "PLAYER_UNGHOST"],
            &["release"]
        ),
        vec!["PLAYER_DEAD | release=-1", "PLAYER_ALIVE | release=-1"]
    );
}

/// A Reincarnation owed at death, its field one flush ahead of health 0: `HasSoulstone`
/// (`0x51acd0`) reads it at call time, so the `DEATH` popup `UIParent` raises on `PLAYER_DEAD`
/// shows its second button with the spell's name.
#[test]
fn a_self_res_owed_at_death_shows_on_the_death_popup() {
    let Some(mut rig) = Rig::login(&[]) else {
        return;
    };
    let seen = rig.packets(
        vec![
            self_values(&[(FIELD_PLAYER_SELF_RES_SPELL, 21169)]),
            self_values(&[(FIELD_UNIT_HEALTH, 0)]),
        ],
        3,
    );
    assert_eq!(
        pin(&seen, &["PLAYER_DEAD"], &["soulstone", "button2"]),
        vec!["PLAYER_DEAD | soulstone=Reincarnation button2=Reincarnation"]
    );
}

/// A `/reload` while dead: the UI load enters the world but never refreshes the dead or alive
/// signal (`0x52a980` calls `0x528010` alone), and it zeroes the release timer (`0x52a9f6`).
#[test]
fn reloading_dead_fires_no_player_dead() {
    let Some(mut rig) = Rig::login(&[]) else {
        return;
    };
    let died = rig.packets(vec![self_values(&[(FIELD_UNIT_HEALTH, 0)])], 3);
    assert_eq!(
        pin(&died, &["PLAYER_DEAD"], &["release"]),
        vec!["PLAYER_DEAD | release=-1"]
    );
    rig.app
        .world()
        .non_send_resource::<benilla_ui::script::UiScript>()
        .run("ReloadUI()")
        .unwrap();
    rig.run(4);
    let seen = rig.take();
    assert_eq!(
        pin(
            &seen,
            &["PLAYER_ENTERING_WORLD", "PLAYER_DEAD", "PLAYER_ALIVE"],
            &["release", "popup"]
        ),
        vec!["PLAYER_ENTERING_WORLD | release=0 popup=nil"]
    );
}

/// A `/reload` zeroes the countdowns the UI load owns (`0x52a9f6`-`0x52aa20`): reclaim, instance
/// boot, instance lock and summon all read 0 after it.
#[test]
fn a_reload_zeroes_the_countdowns() {
    let Some(mut rig) = Rig::login(&[]) else {
        return;
    };
    let armed = rig.packets(
        vec![
            SessionEvent::CorpseReclaimDelay { delay_ms: 30_000 },
            SessionEvent::RaidGroupOnly {
                delay_ms: 60_000,
                reason: 0,
            },
            SessionEvent::InstanceLockWarning {
                warning: benilla_protocol::messages::InstanceLockWarning {
                    ms: 90_000,
                    completed_mask: 0,
                    extending: false,
                },
            },
            SessionEvent::SummonRequest {
                summoner: 0x20,
                zone: 1519,
                delay_ms: 120_000,
            },
        ],
        3,
    );
    assert_eq!(
        pin(&armed, &["CONFIRM_SUMMON"], &["reclaim", "summon"]),
        vec!["CONFIRM_SUMMON | reclaim=30 summon=120"]
    );
    let clocks = |rig: &Rig| {
        rig.app
            .world()
            .non_send_resource::<benilla_ui::script::UiScript>()
            .eval::<String>(
                "return GetInstanceBootTimeRemaining() .. ',' .. GetInstanceLockTimeRemaining()",
            )
            .unwrap()
    };
    assert_eq!(clocks(&rig), "59,89", "the boot and lock clocks run");
    rig.app
        .world()
        .non_send_resource::<benilla_ui::script::UiScript>()
        .run("ReloadUI()")
        .unwrap();
    rig.run(4);
    let seen = rig.take();
    assert_eq!(
        pin(&seen, &["PLAYER_ENTERING_WORLD"], &["reclaim", "summon"]),
        vec!["PLAYER_ENTERING_WORLD | reclaim=0 summon=0"]
    );
    assert_eq!(clocks(&rig), "0,0");
}

/// The summon offer and its withdrawal in one frame and apart: `CONFIRM_SUMMON` fires from the
/// request's handler (`0x6d8680` -> `0x513c21`) with the deadline already latched, which
/// `GetSummonConfirmTimeLeft` reads at call time (`0x5164b0`), and `CANCEL_SUMMON` from the
/// cancel's (`0x6cbcf0`), in wire order.
#[test]
fn summon_offer_and_cancel_fire_from_their_handlers() {
    let Some(mut rig) = Rig::login(&[]) else {
        return;
    };
    let request = || SessionEvent::SummonRequest {
        summoner: 0x20,
        zone: 1519,
        delay_ms: 120_000,
    };
    let events = &["CONFIRM_SUMMON", "CANCEL_SUMMON"];
    let both = rig.packets(vec![request(), SessionEvent::SummonCancel], 3);
    assert_eq!(
        pin(&both, events, &["summon"]),
        vec!["CONFIRM_SUMMON | summon=120", "CANCEL_SUMMON | summon=120"]
    );
    let ask = rig.packets(vec![request()], 3);
    assert_eq!(
        pin(&ask, events, &["summon"]),
        vec!["CONFIRM_SUMMON | summon=120"]
    );
}

/// A named caster's offer while dead asks at once from its handler (`0x6dbd00` -> `0x512511`);
/// alive, the offer only latches.
#[test]
fn a_resurrect_offer_asks_only_while_dead() {
    let Some(mut rig) = Rig::login(&[]) else {
        return;
    };
    let offer = || SessionEvent::ResurrectRequest {
        caster: 0x20,
        name: "Charzpriest".into(),
        sickness: false,
        has_timer: true,
    };
    let alive = rig.packets(vec![offer()], 3);
    assert!(pin(&alive, &["RESURRECT_REQUEST"], &[]).is_empty());
    let dead = rig.packets(vec![self_values(&[(FIELD_UNIT_HEALTH, 0)]), offer()], 3);
    assert_eq!(
        pin(&dead, &["PLAYER_DEAD", "RESURRECT_REQUEST"], &[]),
        vec!["PLAYER_DEAD", "RESURRECT_REQUEST Charzpriest"]
    );
}

/// The flags packet ahead of the control update: 12340 fires `UNIT_FLAGS` from the flags packet's
/// field handler (`0x60c240`), still in control, then the control events from the control
/// update's (`0x520fe0`), and `UnitOnTaxi` reads the flags the earlier packet set.
#[test]
fn flight_start_and_end_with_the_flags_ahead_of_the_control_update() {
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    let events = &["UNIT_FLAGS", "PLAYER_CONTROL_LOST", "PLAYER_CONTROL_GAINED"];
    let verbs = &["taxi", "control"];
    let start = rig.packets(
        vec![flags(crate::player::UNIT_FLAG_TAXI_FLIGHT), control(false)],
        3,
    );
    let end = rig.packets(vec![flags(0), control(true)], 3);
    assert_eq!(
        pin(&start, events, verbs),
        vec![
            "UNIT_FLAGS player | taxi=1 control=1",
            "PLAYER_CONTROL_LOST | taxi=1 control=nil",
        ]
    );
    assert_eq!(
        pin(&end, events, verbs),
        vec![
            "UNIT_FLAGS player | taxi=nil control=nil",
            "PLAYER_CONTROL_GAINED | taxi=nil control=1",
        ]
    );
}

#[test]
fn quest_accept_and_complete() {
    let Some(mut rig) = Rig::login(&[]) else {
        return;
    };
    let events = &[
        "UNIT_QUEST_LOG_CHANGED",
        "QUEST_LOG_UPDATE",
        "QUEST_ACCEPTED",
        "QUEST_WATCH_UPDATE",
    ];
    let verbs = &["quests"];
    let accept = rig.packets(
        vec![
            quest_template(0),
            self_values(&[(FIELD_PLAYER_QUEST_LOG_1_1, QUEST)]),
        ],
        3,
    );
    let complete = rig.packets(
        vec![
            self_values(&[(FIELD_PLAYER_QUEST_LOG_1_1, 0)]),
            SessionEvent::QuestComplete(benilla_protocol::messages::QuestComplete {
                quest_id: QUEST,
                xp: 100,
                ..Default::default()
            }),
        ],
        3,
    );
    // The slot watcher rebuilds the rows (`0x6df3bb`) and fires `QUEST_ACCEPTED` for a cached
    // template at once (`0x6df49c`); the rebuild's `QUEST_LOG_UPDATE` waits for the frame's flush
    // (`0x5e6957`, `0x614760`).
    assert_eq!(
        pin(&accept, events, verbs),
        vec![
            "UNIT_QUEST_LOG_CHANGED player | quests=0",
            "QUEST_ACCEPTED 2 | quests=2",
            "QUEST_LOG_UPDATE | quests=2",
        ]
    );
    assert_eq!(
        pin(&complete, events, verbs),
        vec![
            "UNIT_QUEST_LOG_CHANGED player | quests=2",
            "QUEST_LOG_UPDATE | quests=0",
        ]
    );
}

#[test]
fn pet_summon_and_dismiss() {
    let Some(mut rig) = Rig::login(&[]) else {
        return;
    };
    let events = &["UNIT_PET", "PET_BAR_UPDATE"];
    let verbs = &["pet", "petui"];
    let pet = SessionEvent::ObjectCreate {
        guid: PET,
        kind: EntityKind::Unit,
        display_id: None,
        position: [-8949.0, -132.0, 83.5],
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
        fields: ObjectFields::from_pairs(&[
            (FIELD_UNIT_HEALTH, 500),
            (FIELD_UNIT_MAXHEALTH, 500),
            (FIELD_UNIT_LEVEL, 80),
        ])
        .into_created(ObjectType::Unit),
    };
    let summon = rig.packets(
        vec![
            pet,
            self_values(&[
                (FIELD_UNIT_SUMMON, PET as u32),
                (FIELD_UNIT_SUMMON + 1, (PET >> 32) as u32),
            ]),
            SessionEvent::PetSpells(Box::new(benilla_protocol::messages::PetSpells {
                pet_guid: PET,
                ..Default::default()
            })),
        ],
        3,
    );
    let dismiss = rig.packets(
        vec![
            self_values(&[(FIELD_UNIT_SUMMON, 0), (FIELD_UNIT_SUMMON + 1, 0)]),
            SessionEvent::PetSpells(Box::default()),
            SessionEvent::ObjectDestroyed(PET),
        ],
        3,
    );
    assert_eq!(
        pin(&summon, events, verbs),
        // The pet unit feeds before the bar (#1244), so the bar's handler sees the pet.
        vec![
            "UNIT_PET player | pet=1 petui=nil",
            "PET_BAR_UPDATE | pet=1 petui=nil",
        ]
    );
    assert_eq!(
        pin(&dismiss, events, verbs),
        vec![
            "UNIT_PET player | pet=nil petui=nil",
            "PET_BAR_UPDATE | pet=nil petui=nil",
        ]
    );
}

#[test]
fn mount_and_dismount() {
    let Some(mut rig) = Rig::login(&[]) else {
        return;
    };
    let verbs = &["mounted"];
    let all = |seen: &[Seen]| seen.iter().map(|s| s.line(verbs)).collect::<Vec<_>>();
    let mount = rig.packets(vec![self_values(&[(FIELD_UNIT_MOUNTDISPLAYID, 2404)])], 3);
    let on = rig.now().line(verbs);
    let dismount = rig.packets(vec![self_values(&[(FIELD_UNIT_MOUNTDISPLAYID, 0)])], 3);
    let off = rig.now().line(verbs);
    // No event fires for either edge; only a later read sees it.
    assert_eq!(
        (all(&mount), on.as_str()),
        (Vec::<String>::new(), "NOW | mounted=1")
    );
    assert_eq!(
        (all(&dismount), off.as_str()),
        (Vec::<String>::new(), "NOW | mounted=nil")
    );
}

/// The area lookup is the client's own (the terrain under the player), so the scenario sets its
/// answer where the lookup writes it.
#[test]
fn zone_change() {
    let Some(mut rig) = Rig::login(&[]) else {
        return;
    };
    let events = &[
        "ZONE_CHANGED_NEW_AREA",
        "ZONE_CHANGED",
        "ZONE_CHANGED_INDOORS",
    ];
    let verbs = &["zone"];
    rig.take();
    let area = |rig: &mut Rig, id: u32| {
        rig.app
            .world_mut()
            .insert_resource(benilla_world::terrain_stream::CurrentArea(Some(id)));
        rig.run(3);
        rig.take()
    };
    let elwynn = area(&mut rig, 12);
    let stormwind = area(&mut rig, 1519);
    assert_eq!(
        pin(&elwynn, events, verbs),
        vec!["ZONE_CHANGED_NEW_AREA | zone=Elwynn Forest",]
    );
    assert_eq!(
        pin(&stormwind, events, verbs),
        vec!["ZONE_CHANGED_NEW_AREA | zone=Stormwind City",]
    );
}

/// `UnitAffectingCombat` reads the live flags word (`0x60f8ac`), so the create names its type.
#[test]
fn combat_enter_and_leave() {
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    let events = &[
        "PLAYER_REGEN_DISABLED",
        "PLAYER_REGEN_ENABLED",
        "UNIT_FLAGS",
    ];
    let verbs = &["combat", "lockdown"];
    let enter = rig.packets(vec![flags(crate::player::UNIT_FLAG_IN_COMBAT)], 3);
    let leave = rig.packets(vec![flags(0)], 3);
    assert_eq!(
        pin(&enter, events, verbs),
        vec![
            "UNIT_FLAGS player | combat=1 lockdown=nil",
            "PLAYER_REGEN_DISABLED | combat=1 lockdown=nil",
        ]
    );
    assert_eq!(
        pin(&leave, events, verbs),
        vec![
            "UNIT_FLAGS player | combat=nil lockdown=1",
            "PLAYER_REGEN_ENABLED | combat=nil lockdown=nil",
        ]
    );
}

/// A death in combat: the server clears the in-combat flag in the update that zeroes health, and
/// 12340 runs the mirror handlers in field order (`0x4d5550`), so health's `PLAYER_DEAD`
/// (`0x73f330`) fires before the flags word's `UNIT_FLAGS` (`0x60c240`).
#[test]
fn a_death_in_combat_fires_player_dead_before_unit_flags() {
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    rig.packets(vec![flags(crate::player::UNIT_FLAG_IN_COMBAT)], 3);
    let seen = rig.packets(
        vec![self_values(&[
            (FIELD_UNIT_HEALTH, 0),
            (FIELD_UNIT_FLAGS, 0),
        ])],
        3,
    );
    assert_eq!(
        pin(&seen, &["PLAYER_DEAD", "UNIT_FLAGS"], &[]),
        vec!["PLAYER_DEAD", "UNIT_FLAGS player"]
    );
}

// ── The known order bugs: the 12340 result, ignored until the migration (#1245) un-ignores ─────

/// Logged in dead inside an arena, the battlefield status on the wire ahead of our create: 12340
/// handles the status first, so `IsActiveBattlefieldArena` (`0x549ad0`, the state word at
/// `0xbea570`) already answers true when our create's init fires `PLAYER_DEAD` after
/// `PLAYER_ENTERING_WORLD` (`0x6e818b`). The UI load zeroed the release timer (`0x52a9f6`) and no
/// death edge armed it, so `GetReleaseTimeRemaining` reads 0 and `UIParent`'s handler shows no
/// `DEATH` popup; the entry sends the automatic release instead (`0x528010` -> `0x6d2950` with 1).
#[test]
fn logging_in_dead_in_an_arena_fires_player_dead_with_the_arena_up() {
    use benilla_protocol::messages::{BattlefieldActive, BattlefieldStatus};
    // A 2v2 queue word (team size 2, type 6: all arenas), active on Nagrand Arena, map 559.
    let status = SessionEvent::BattlefieldStatus(BattlefieldStatus {
        slot: 0,
        queue: (6 << 16) | 2,
        status: 3,
        active: Some(BattlefieldActive {
            map_id: 559,
            joined_queue: 0,
            shutdown_ms: 0,
            run_ms: 30_000,
            arena_faction: 0,
        }),
        ..Default::default()
    });
    let Some(mut rig) = Rig::login_after(vec![status], &[(FIELD_UNIT_HEALTH, 0)]) else {
        return;
    };
    let seen = rig.take();
    assert_eq!(
        pin(
            &seen,
            &["PLAYER_ENTERING_WORLD", "PLAYER_DEAD", "PLAYER_ALIVE"],
            &["arena", "release", "popup"]
        ),
        vec![
            "PLAYER_ENTERING_WORLD | arena=1 release=0 popup=nil",
            "PLAYER_DEAD | arena=1 release=0 popup=nil",
        ]
    );
    assert_eq!(rig.now().verbs["popup"], "nil");
}

/// The control update ahead of the flags in one frame: 12340 fires `PLAYER_CONTROL_LOST` inside
/// the control update's handler (`0x520fe0`), before the flags packet is handled, so `UnitOnTaxi`
/// (`0x60f350`, bit 20 of the live flags word at `0x60f390`-`0x60f39f`) still answers false there,
/// and `PLAYER_CONTROL_GAINED` at the end still finds the ride.
#[test]
fn flight_start_with_the_control_update_ahead_of_the_flags() {
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    let events = &["PLAYER_CONTROL_LOST", "PLAYER_CONTROL_GAINED"];
    let start = rig.packets(
        vec![control(false), flags(crate::player::UNIT_FLAG_TAXI_FLIGHT)],
        3,
    );
    let end = rig.packets(vec![control(true), flags(0)], 3);
    assert_eq!(
        pin(&start, events, &["taxi", "control"]),
        vec!["PLAYER_CONTROL_LOST | taxi=nil control=nil".to_string()]
    );
    assert_eq!(
        pin(&end, events, &["taxi", "control"]),
        vec!["PLAYER_CONTROL_GAINED | taxi=1 control=1".to_string()]
    );
}

/// Accepting a quest whose source item is already carried: 12340's `GetQuestLogSpecialItemInfo`
/// (`0x5e52d0`) reads the quest and the bags at call time, so the tracker's `QUEST_LOG_UPDATE`
/// redraw already finds the special item.
#[test]
fn quest_accept_shows_the_special_item_with_the_log_update() {
    let Some((_, accept)) = accept_with_the_source_item() else {
        return;
    };
    let first = accept
        .iter()
        .find(|s| s.event == "QUEST_LOG_UPDATE")
        .expect("QUEST_LOG_UPDATE");
    assert_eq!(first.verbs["special"], "true");
}

/// The accept's `QUEST_ACCEPTED` (`0x6df49c`) and the flushed `QUEST_LOG_UPDATE` after it
/// (`0x614760`) both read the special item: the verb reads the quest and the bags when called
/// (`0x5e0da0`), as do the calls after the frames.
#[test]
fn quest_accept_with_a_carried_source_item() {
    let Some((mut rig, accept)) = accept_with_the_source_item() else {
        return;
    };
    assert_eq!(
        pin(
            &accept,
            &["QUEST_LOG_UPDATE", "QUEST_ACCEPTED"],
            &["special"]
        ),
        vec![
            "QUEST_ACCEPTED 2 | special=true",
            "QUEST_LOG_UPDATE | special=true",
        ]
    );
    assert_eq!(rig.live_now().verbs["special"], "true");
}

/// Two POI responses in one frame: each handler writes the store in turn (`0x5e7370`) and queues
/// `QUEST_POI_UPDATE` as a unique signal (`0x5e7628`), which fires once, at the frame's flush
/// (`0x614760`), never per response.
#[test]
fn two_poi_responses_in_one_frame_fire_one_poi_update() {
    let Some(mut rig) = Rig::login(&[]) else {
        return;
    };
    let seen = rig.packets(vec![poi(QUEST), poi(QUEST + 1)], 3);
    assert_eq!(
        pin(&seen, &["QUEST_POI_UPDATE"], &[]),
        vec!["QUEST_POI_UPDATE"]
    );
}

fn poi(quest_id: u32) -> SessionEvent {
    SessionEvent::QuestPoi(vec![benilla_protocol::messages::QuestPoi {
        quest_id,
        blobs: Vec::new(),
    }])
}

fn server_time() -> SessionEvent {
    SessionEvent::ServerUnixTime {
        unix_time: 1_000_000,
        reset_in: 3_600,
    }
}

/// What [`charz_poi_seen`] read of the POI store, one answer per control update.
#[derive(Resource, Clone, Default)]
struct PoiSeen(std::sync::Arc<std::sync::Mutex<Vec<bool>>>);

/// A test handler on the control update reading whether the POI store holds [`QUEST`] at its turn.
fn charz_poi_seen(
    In(ev): In<SessionEvent>,
    mut ui: ResMut<crate::ui_script::live::UiEvents>,
    seen: Res<PoiSeen>,
) {
    if let SessionEvent::ClientControl { .. } = ev {
        let seen = seen.0.clone();
        ui.run(move |s| {
            seen.lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(s.quest_poi_held(QUEST))
        });
    }
}

/// The POI response writes its store from the handler (`0x5e7370`), so the next packet's handler
/// in the same frame already finds it; nothing waits for a per-frame pass.
#[test]
fn the_next_packets_handler_finds_the_poi_response_stored() {
    use crate::net::handlers::NetHandlerApp;
    let Some(mut rig) = Rig::login(&[]) else {
        return;
    };
    let seen = PoiSeen::default();
    rig.app.insert_resource(seen.clone());
    rig.app.net_handler(
        benilla_protocol::SessionEventKind::ClientControl,
        charz_poi_seen,
    );
    rig.packets(vec![control(true)], 1);
    rig.packets(vec![poi(QUEST), control(true)], 1);
    assert_eq!(*seen.0.lock().unwrap(), vec![false, true]);
}

/// `QUEST_POI_UPDATE` (`0x5e7628`) and the time response's `QUEST_LOG_UPDATE` (`0x5dff89`) are
/// queued by their handlers and flushed in queue order (`0x614760`): wire order, either way round.
#[test]
fn the_poi_and_time_responses_signal_in_wire_order() {
    let Some(mut rig) = Rig::login(&[]) else {
        return;
    };
    let events = &["QUEST_POI_UPDATE", "QUEST_LOG_UPDATE"];
    let poi_first = rig.packets(vec![poi(QUEST), server_time()], 3);
    let time_first = rig.packets(vec![server_time(), poi(QUEST + 1)], 3);
    assert_eq!(
        pin(&poi_first, events, &[]),
        vec!["QUEST_POI_UPDATE", "QUEST_LOG_UPDATE"]
    );
    assert_eq!(
        pin(&time_first, events, &[]),
        vec!["QUEST_LOG_UPDATE", "QUEST_POI_UPDATE"]
    );
}

/// The quest-slot watch rebuilds the log (`0x6df3bb`), queueing `QUEST_LOG_UPDATE` (`0x5e6957`)
/// from the update handler, so it keeps wire order with a POI response's `QUEST_POI_UPDATE`
/// (`0x5e7628`) at the flush (`0x614760`).
#[test]
fn the_slot_watch_and_the_poi_response_signal_in_wire_order() {
    let Some(mut rig) = Rig::login(&[]) else {
        return;
    };
    let events = &["QUEST_POI_UPDATE", "QUEST_LOG_UPDATE"];
    rig.packets(vec![quest_template(0)], 3);
    let slot_first = rig.packets(
        vec![
            self_values(&[(FIELD_PLAYER_QUEST_LOG_1_1, QUEST)]),
            poi(QUEST),
        ],
        3,
    );
    let poi_first = rig.packets(
        vec![
            poi(QUEST + 1),
            self_values(&[(FIELD_PLAYER_QUEST_LOG_1_1, 0)]),
        ],
        3,
    );
    assert_eq!(
        pin(&slot_first, events, &["quests"]),
        vec!["QUEST_LOG_UPDATE | quests=2", "QUEST_POI_UPDATE | quests=2"]
    );
    assert_eq!(
        pin(&poi_first, events, &["quests"]),
        vec!["QUEST_POI_UPDATE | quests=0", "QUEST_LOG_UPDATE | quests=0"]
    );
}

/// The time response sets the offset and the reset (`0x5dff40`) and queues `QUEST_LOG_UPDATE`
/// (`0x5dff89`); `GetQuestResetTime` reads them when called (`0x5e6de0`), 0 before any response.
#[test]
fn the_time_response_updates_the_log_with_the_reset_already_read() {
    let Some(mut rig) = Rig::login(&[]) else {
        return;
    };
    assert_eq!(rig.live_now().verbs["reset"], "0");
    let seen = rig.packets(vec![server_time()], 3);
    assert_eq!(
        pin(&seen, &["QUEST_LOG_UPDATE"], &["reset"]),
        vec!["QUEST_LOG_UPDATE | reset=3600"]
    );
    assert_eq!(rig.live_now().verbs["reset"], "3600");
}

/// A logged-in rig carrying an on-use item in the backpack's first slot, then the accept of the
/// quest whose source item it is; what the accept's frames saw.
fn accept_with_the_source_item() -> Option<(Rig, Vec<Seen>)> {
    const ITEM: u32 = 5000;
    const ITEM_GUID: u64 = 0x4000_0000_0000_0100;
    let mut rig = Rig::login(&[])?;
    let mut info = crate::items::test_template("Charz Relic");
    info.use_spell = Some(benilla_protocol::messages::ItemUseSpell {
        spell_id: 1000,
        cooldown_ms: -1,
        category: 0,
        category_cooldown_ms: -1,
    });
    rig.packets(
        vec![
            SessionEvent::ItemTemplate {
                entry: ITEM,
                info: Some(Box::new(info)),
            },
            SessionEvent::ItemCreate {
                guid: ITEM_GUID,
                container: false,
                // `OBJECT_FIELD_ENTRY`, word 3.
                fields: ObjectFields::from_pairs(&[(
                    benilla_protocol::field::FIELD_OBJECT_ENTRY,
                    ITEM,
                )])
                .into_created(ObjectType::Item),
            },
            // The backpack's first slot, after the 23 equipment and bag slots.
            self_values(&[
                (FIELD_PLAYER_INV_SLOT_HEAD + 46, ITEM_GUID as u32),
                (FIELD_PLAYER_INV_SLOT_HEAD + 47, (ITEM_GUID >> 32) as u32),
            ]),
        ],
        3,
    );
    let accept = rig.packets(
        vec![
            quest_template(ITEM),
            self_values(&[(FIELD_PLAYER_QUEST_LOG_1_1, QUEST)]),
        ],
        3,
    );
    Some((rig, accept))
}

// ── The live infrastructure end to end ───────────────────────────────────────────────────────────

/// A test handler on the control update firing `CHARZ_HANDLER`, as the reference's handler fires
/// its event (`0x520fe0`).
fn charz_control(In(ev): In<SessionEvent>, mut ui: ResMut<crate::ui_script::live::UiEvents>) {
    if let SessionEvent::ClientControl { allow_move, .. } = ev {
        let edge = if allow_move { "gained" } else { "lost" };
        ui.fire(
            "CHARZ_HANDLER",
            vec![benilla_ui::script::ScriptValue::Str(edge.into())],
        );
    }
}

/// A logged-in rig with [`charz_control`] registered after the stock control handler, the test verb
/// `CharzLiveTaxi` installed, and a frame logging each `CHARZ_HANDLER` with our taxi bit as the
/// lent view reads it and as `UnitOnTaxi` answers it.
fn login_with_handler_event() -> Option<Rig> {
    use crate::net::handlers::NetHandlerApp;
    let mut rig = Rig::login(&[OBJECT_TYPE_PLAYER])?;
    rig.app.net_handler(
        benilla_protocol::SessionEventKind::ClientControl,
        charz_control,
    );
    let script = rig
        .app
        .world()
        .non_send_resource::<benilla_ui::script::UiScript>();
    super::live_tests::install_live_taxi(script);
    script
        .run(&format!(
            r#"
            CHARZ_LIVE = {{}}
            local f = CreateFrame("Frame")
            f:RegisterEvent("CHARZ_HANDLER")
            f:SetScript("OnEvent", function(_, _, edge)
                CHARZ_LIVE[#CHARZ_LIVE + 1] = edge .. " live=" .. CharzLiveTaxi({ME})
                    .. " verb=" .. tostring(UnitOnTaxi("player"))
            end)
            "#
        ))
        .unwrap();
    Some(rig)
}

/// The `CHARZ_LIVE` lines since the last call.
fn take_live(rig: &Rig) -> Vec<String> {
    rig.app
        .world()
        .non_send_resource::<benilla_ui::script::UiScript>()
        .eval("local t = CHARZ_LIVE; CHARZ_LIVE = {}; return t")
        .unwrap()
}

/// A handler-fired event reaches Lua inside the drain, in wire order: after every earlier packet's
/// change (the flags ahead of it), before any later one's (the flags behind it), and a verb its
/// handler calls reads the `World` through the lent view, `UnitOnTaxi` among them.
#[test]
fn a_handler_event_sees_the_packets_before_it_and_none_after() {
    let Some(mut rig) = login_with_handler_event() else {
        return;
    };
    let taxi = crate::player::UNIT_FLAG_TAXI_FLIGHT;
    let ahead = rig.packets(vec![flags(taxi), control(false)], 1);
    assert_eq!(take_live(&rig), vec!["lost live=1 verb=1"]);
    rig.packets(vec![flags(0), control(true)], 1);
    assert_eq!(take_live(&rig), vec!["gained live=nil verb=nil"]);
    rig.packets(vec![control(false), flags(taxi)], 1);
    assert_eq!(take_live(&rig), vec!["lost live=nil verb=nil"]);
    rig.packets(vec![flags(0), control(true), flags(taxi)], 1);
    assert_eq!(take_live(&rig), vec!["gained live=nil verb=nil"]);
    // The observer sees it too, inside the drain ahead of every feed, after the stock control
    // handler registered before it and the flags packet's `UNIT_FLAGS` (`0x60c240`) ahead of both.
    assert_eq!(
        pin(&ahead, &["CHARZ_HANDLER"], &["taxi"]),
        vec!["CHARZ_HANDLER lost | taxi=1"]
    );
    let first: Vec<&str> = ahead.iter().take(3).map(|s| s.event.as_str()).collect();
    assert_eq!(
        first,
        ["UNIT_FLAGS", "PLAYER_CONTROL_LOST", "CHARZ_HANDLER"]
    );
}

/// The OnUpdate tick lends the view too: a verb there reads the flags the frame's packet set;
/// outside any call into Lua nothing is lent.
#[test]
fn an_on_update_reads_the_world_through_the_lent_view() {
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    let script = rig
        .app
        .world()
        .non_send_resource::<benilla_ui::script::UiScript>();
    super::live_tests::install_live_taxi(script);
    script
        .run(&format!(
            r#"
            CHARZ_TICK = {{}}
            local f = CreateFrame("Frame")
            f:SetScript("OnUpdate", function()
                CHARZ_TICK[#CHARZ_TICK + 1] = CharzLiveTaxi({ME})
            end)
            "#
        ))
        .unwrap();
    let tick = |rig: &mut Rig| -> Vec<String> {
        rig.app
            .world()
            .non_send_resource::<benilla_ui::script::UiScript>()
            .eval("local t = CHARZ_TICK; CHARZ_TICK = {}; return t")
            .unwrap()
    };
    rig.packets(vec![flags(crate::player::UNIT_FLAG_TAXI_FLIGHT)], 1);
    assert_eq!(tick(&mut rig), vec!["1"]);
    rig.packets(vec![flags(0)], 1);
    assert_eq!(tick(&mut rig), vec!["nil"]);
    let outside: String = rig
        .app
        .world()
        .non_send_resource::<benilla_ui::script::UiScript>()
        .eval(&format!("return CharzLiveTaxi({ME})"))
        .unwrap();
    assert_eq!(outside, "unlent");
}

/// A feed lends the view for its calls into Lua: the `UNIT_HEALTH` the unit feed fires beside a
/// taxi flags change reads the `World` there, never an unlent view, and reads the flags the
/// frame's packets set.
#[test]
fn a_feed_fired_event_reads_the_world_through_the_lent_view() {
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    let script = rig
        .app
        .world()
        .non_send_resource::<benilla_ui::script::UiScript>();
    super::live_tests::install_live_taxi(script);
    script
        .run(&format!(
            r#"
            CHARZ_FEED = {{}}
            local f = CreateFrame("Frame")
            f:RegisterEvent("UNIT_HEALTH")
            f:SetScript("OnEvent", function(_, e, unit)
                CHARZ_FEED[#CHARZ_FEED + 1] = e .. " " .. tostring(unit) .. " live="
                    .. CharzLiveTaxi({ME})
            end)
            "#
        ))
        .unwrap();
    let feed = |rig: &mut Rig| -> Vec<String> {
        rig.app
            .world()
            .non_send_resource::<benilla_ui::script::UiScript>()
            .eval("local t = CHARZ_FEED; CHARZ_FEED = {}; return t")
            .unwrap()
    };
    rig.packets(
        vec![self_values(&[
            (FIELD_UNIT_FLAGS, crate::player::UNIT_FLAG_TAXI_FLIGHT),
            (FIELD_UNIT_HEALTH, 900),
        ])],
        1,
    );
    assert_eq!(feed(&mut rig), vec!["UNIT_HEALTH player live=1"]);
    rig.packets(
        vec![self_values(&[
            (FIELD_UNIT_FLAGS, 0),
            (FIELD_UNIT_HEALTH, 800),
        ])],
        1,
    );
    assert_eq!(feed(&mut rig), vec!["UNIT_HEALTH player live=nil"]);
}

/// The death verbs read the `World` when called: a change made between frames, before any system
/// runs, reads at once in `IsFalling` (`0x612430`, the mover's flags word), `HasSoulstone`
/// (`0x51acd0`), `CannotBeResurrected` (`0x802270`, our aura effects), `CheckSpiritHealerDist`
/// (`0x6d1fc0`, both positions) and `ResurrectHasTimer`'s arena test (`0x515a00`, `[0xbea570]`).
#[test]
fn the_death_verbs_read_the_world_between_frames() {
    use crate::creature_anim::move_flags::{FALLING, ROOT};
    use benilla_protocol::messages::{BattlefieldActive, BattlefieldStatus};
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    let me = |rig: &mut Rig| {
        let world = rig.app.world_mut();
        world
            .query_filtered::<Entity, With<crate::net::SelfPlayer>>()
            .single(world)
            .expect("our player")
    };
    let read = |rig: &mut Rig, verb: &str| rig.live_now().verbs[verb].clone();

    let fall = |rig: &mut Rig, flags: u32| {
        rig.app
            .world_mut()
            .resource_mut::<crate::player::Player>()
            .set_move_flags(flags);
    };
    fall(&mut rig, FALLING);
    assert_eq!(read(&mut rig, "falling"), "1", "the fall reads at once");
    fall(&mut rig, FALLING | ROOT);
    assert_eq!(read(&mut rig, "falling"), "nil", "a rooted fall is no fall");
    fall(&mut rig, 0);

    assert_eq!(read(&mut rig, "soulstone"), "nil");
    let entity = me(&mut rig);
    rig.app
        .world_mut()
        .get_mut::<crate::net::ObjectStore>(entity)
        .unwrap()
        .0
        .merge(ObjectFields::from_pairs(&[
            (FIELD_PLAYER_SELF_RES_SPELL, 21169),
            (FIELD_UNIT_HEALTH, 0),
        ]));
    assert_eq!(
        read(&mut rig, "soulstone"),
        "Reincarnation",
        "dead with one owed"
    );

    assert_eq!(read(&mut rig, "nores"), "nil");
    let (spell, effect) = rig
        .app
        .world()
        .resource::<crate::ui_action::Spells>()
        .catalog
        .iter()
        .find_map(|(id, d)| {
            let i = d.effect_apply_aura.iter().position(|&a| a == 314)?;
            Some((id, i))
        })
        .expect("a spell applying aura 314");
    rig.app
        .world_mut()
        .entity_mut(entity)
        .insert(crate::unit_auras::UnitAuras::of(
            ME,
            &[(0, spell, 1 << effect)],
        ));
    assert_eq!(
        read(&mut rig, "nores"),
        "1",
        "the aura's effect reads at once"
    );

    const HEALER: u64 = 0xF130_0000_0000_6767;
    rig.packets(
        vec![SessionEvent::ObjectCreate {
            guid: HEALER,
            kind: EntityKind::Unit,
            display_id: None,
            position: [-8949.0, -132.0, 83.5],
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
            // `OBJECT_FIELD_TYPE` object and unit, which the verbs' unit lookup tests.
            fields: ObjectFields::from_pairs(&[
                (2, 0x9),
                (FIELD_UNIT_HEALTH, 500),
                (FIELD_UNIT_MAXHEALTH, 500),
                (FIELD_UNIT_LEVEL, 80),
            ])
            .into_created(ObjectType::Unit),
        }],
        2,
    );
    let place = |rig: &mut Rig, yards: f32| {
        let e = me(rig);
        let world = rig.app.world_mut();
        let at = world.get::<Transform>(e).unwrap().translation;
        let healer = world.resource::<crate::net::GuidIndex>().0[&HEALER];
        world.get_mut::<Transform>(healer).unwrap().translation = at + Vec3::X * yards;
        world
            .resource_mut::<crate::death::DeathGlobals>()
            .spirit_healer = Some(HEALER);
    };
    place(&mut rig, 3.0);
    assert_eq!(
        read(&mut rig, "healer"),
        "1",
        "the healer stands within its reach"
    );
    place(&mut rig, 50.0);
    assert_eq!(
        read(&mut rig, "healer"),
        "nil",
        "and its move out reads at once"
    );

    rig.app
        .world_mut()
        .resource_mut::<crate::death::DeathGlobals>()
        .resurrect = Some(crate::death::ResurrectOffer {
        caster: 0x20,
        name: "Brisca".into(),
        sickness: false,
        has_timer: true,
        awaiting_name: false,
    });
    assert_eq!(read(&mut rig, "restimer"), "1");
    // A 2v2 queue word, active on Nagrand Arena, map 559.
    rig.app
        .world_mut()
        .resource_mut::<crate::ui_battlefield::BattlefieldQueue>()
        .apply(BattlefieldStatus {
            slot: 0,
            queue: (6 << 16) | 2,
            status: 3,
            active: Some(BattlefieldActive {
                map_id: 559,
                joined_queue: 0,
                shutdown_ms: 0,
                run_ms: 30_000,
                arena_faction: 0,
            }),
            ..Default::default()
        });
    assert_eq!(
        read(&mut rig, "restimer"),
        "nil",
        "inside an arena the offer has no timer"
    );
}
