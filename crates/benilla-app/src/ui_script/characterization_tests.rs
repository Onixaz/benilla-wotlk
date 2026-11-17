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
    { "bf1", function() return (GetBattlefieldStatus(1)) end },
    { "bfrun", function() return GetBattlefieldInstanceRunTime() > 0 end },
    { "scores", function() return GetNumBattlefieldScores() end },
    { "winner", function() return GetBattlefieldWinner() end },
    { "score1", function() return (GetBattlefieldScore(1)) end },
    { "bfcount", function() return GetNumBattlefields() end },
    { "wpvp", function() return (GetWorldPVPQueueStatus(1)) end },
    { "opps", function() return GetNumArenaOpponents() end },
    { "a1name", function() return (UnitName("arena1")) end },
    { "kills", function() return (GetPVPSessionStats()) end },
    { "honorpts", function() return (GetHonorCurrency()) end },
    { "pvptimer", function() return GetPVPTimer() end },
    { "inspecthonor", function() return HasInspectHonorData() end },
    { "inteam", function() return IsInArenaTeam() end },
    { "team1", function() return (GetArenaTeam(1)) end },
    { "rating1", function() return (select(3, GetArenaTeam(1))) end },
    { "members1", function() return GetNumArenaTeamMembers(1) end },
    { "inspteam", function() return (GetInspectArenaTeamData(1)) end },
    { "a2name", function() return (UnitName("arena2")) end },
    { "bfpos", function() return GetNumBattlefieldPositions() end },
    { "bfpos1", function() return (select(3, GetBattlefieldPosition(1))) end },
    { "bfstats", function() return GetNumBattlefieldStats() end },
    { "ap1", function() return UnitExists("arenapet1") end },
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
    assert_eq!(
        pin(&accept, events, verbs),
        vec![
            "UNIT_QUEST_LOG_CHANGED player | quests=0",
            "QUEST_LOG_UPDATE | quests=2",
            "QUEST_ACCEPTED 2 | quests=2",
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
#[ignore = "#1245: feed_special_items pushes after feed_quest_log has fired QUEST_LOG_UPDATE"]
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

/// Today: the accept's `QUEST_LOG_UPDATE` reads no special item; `QUEST_ACCEPTED`, fired after it, does.
#[test]
fn quest_accept_with_a_carried_source_item() {
    let Some((rig, accept)) = accept_with_the_source_item() else {
        return;
    };
    assert_eq!(
        pin(
            &accept,
            &["QUEST_LOG_UPDATE", "QUEST_ACCEPTED"],
            &["special"]
        ),
        vec![
            "QUEST_LOG_UPDATE | special=false",
            "QUEST_ACCEPTED 2 | special=true",
        ]
    );
    assert_eq!(rig.now().verbs["special"], "true");
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
        .apply(
            BattlefieldStatus {
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
            },
            Some(4),
        );
    assert_eq!(
        read(&mut rig, "restimer"),
        "nil",
        "inside an arena the offer has no timer"
    );
}

/// A slot queued then active on an arena map in one frame: each status fires its event as it applies (`0x54ae40`, `0x54b1a9`).
#[test]
fn battlefield_statuses_in_one_frame_fire_in_wire_order() {
    use benilla_protocol::messages::{BattlefieldActive, BattlefieldStatus};
    let Some(mut rig) = Rig::login(&[]) else {
        return;
    };
    let queue = (6 << 16) | 2;
    let queued = SessionEvent::BattlefieldStatus(BattlefieldStatus {
        slot: 0,
        queue,
        status: 1,
        queued: Some((60_000, 0)),
        ..Default::default()
    });
    let active = SessionEvent::BattlefieldStatus(BattlefieldStatus {
        slot: 0,
        queue,
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
    let seen = rig.packets(vec![queued, active], 2);
    assert_eq!(
        pin(
            &seen,
            &["UPDATE_BATTLEFIELD_STATUS", "UPDATE_BATTLEFIELD_SCORE"],
            &["bf1", "arena", "bfrun"]
        ),
        vec![
            "UPDATE_BATTLEFIELD_STATUS 1 | bf1=queued arena=nil bfrun=false",
            "UPDATE_BATTLEFIELD_SCORE | bf1=active arena=1 bfrun=true",
            "UPDATE_BATTLEFIELD_STATUS 1 | bf1=active arena=1 bfrun=true",
        ]
    );
}

/// The duel's packets around a status fire in wire order (`0x5cf910`, `0x54ae40`, `0x5cf930`).
#[test]
fn duel_and_battlefield_events_fire_in_wire_order() {
    use benilla_protocol::messages::BattlefieldStatus;
    let Some(mut rig) = Rig::login(&[]) else {
        return;
    };
    let queued = SessionEvent::BattlefieldStatus(BattlefieldStatus {
        slot: 1,
        queue: (2 << 16) | 0x80,
        status: 1,
        queued: Some((60_000, 0)),
        ..Default::default()
    });
    let seen = rig.packets(
        vec![
            SessionEvent::DuelRequested {
                arbiter: 0xF110_0000_0000_0001,
                challenger: 0x99,
            },
            SessionEvent::DuelOutOfBounds,
            queued,
            SessionEvent::DuelInBounds,
            SessionEvent::DuelComplete { started: true },
        ],
        2,
    );
    assert_eq!(
        pin(
            &seen,
            &[
                "DUEL_REQUESTED",
                "DUEL_OUTOFBOUNDS",
                "UPDATE_BATTLEFIELD_STATUS",
                "DUEL_INBOUNDS",
                "DUEL_FINISHED"
            ],
            &[]
        ),
        vec![
            "DUEL_OUTOFBOUNDS",
            "UPDATE_BATTLEFIELD_STATUS 2",
            "DUEL_INBOUNDS",
            "DUEL_FINISHED",
        ],
        "a challenger no object holds raises no popup (`0x5cfd73`)"
    );
}

/// The event names of `seen` that are in `events`, in order.
fn fired(seen: &[Seen], events: &[&str]) -> Vec<String> {
    seen.iter()
        .filter(|s| events.contains(&s.event.as_str()))
        .map(|s| s.event.clone())
        .collect()
}

/// A queued status on the first slot.
fn queued_status() -> SessionEvent {
    SessionEvent::BattlefieldStatus(benilla_protocol::messages::BattlefieldStatus {
        slot: 0,
        queue: (2 << 16) | 0x80,
        status: 1,
        queued: Some((60_000, 0)),
        ..Default::default()
    })
}

/// The port denial (`0x54d4d4`) and the join verdict (`0x54b1c0`) show their lines through the message display in their handler's turn (`0x52188a`, `0x521863`), around a status in either order.
#[test]
fn battlefield_handler_lines_fire_in_wire_order() {
    let Some(mut rig) = Rig::login(&[]) else {
        return;
    };
    let events = [
        "UI_ERROR_MESSAGE",
        "UI_INFO_MESSAGE",
        "UPDATE_BATTLEFIELD_STATUS",
    ];
    let too_many = SessionEvent::GroupJoinedBattleground(
        benilla_protocol::messages::GroupJoinedBattleground {
            result: -4,
            player: None,
        },
    );
    let deserters = SessionEvent::GroupJoinedBattleground(
        benilla_protocol::messages::GroupJoinedBattleground {
            result: -2,
            player: None,
        },
    );
    let seen = rig.packets(
        vec![
            SessionEvent::BattlefieldPortDenied,
            queued_status(),
            too_many,
            deserters,
        ],
        2,
    );
    assert_eq!(
        fired(&seen, &events),
        [
            "UI_ERROR_MESSAGE",
            "UPDATE_BATTLEFIELD_STATUS",
            "UI_ERROR_MESSAGE",
            "UI_INFO_MESSAGE",
        ]
    );
    let seen = rig.packets(
        vec![queued_status(), SessionEvent::BattlefieldPortDenied],
        2,
    );
    assert_eq!(
        fired(&seen, &events),
        ["UPDATE_BATTLEFIELD_STATUS", "UI_ERROR_MESSAGE"]
    );
}

/// An unstarted duel's completion shows `ERR_DUEL_CANCELLED` (`0x5cfabe`, an info line) before `DUEL_FINISHED` (`0x5cfad8`), both ahead of a later status.
#[test]
fn a_cancelled_duel_shows_its_line_before_it_finishes() {
    let Some(mut rig) = Rig::login(&[]) else {
        return;
    };
    let seen = rig.packets(
        vec![
            SessionEvent::DuelRequested {
                arbiter: 0xF110_0000_0000_0001,
                challenger: 0x99,
            },
            SessionEvent::DuelComplete { started: false },
            queued_status(),
        ],
        2,
    );
    assert_eq!(
        fired(
            &seen,
            &[
                "UI_INFO_MESSAGE",
                "DUEL_FINISHED",
                "UPDATE_BATTLEFIELD_STATUS"
            ]
        ),
        [
            "UI_INFO_MESSAGE",
            "DUEL_FINISHED",
            "UPDATE_BATTLEFIELD_STATUS"
        ]
    );
}

/// A held player of `guid`, as a stream create sends it.
fn player_create(guid: u64) -> SessionEvent {
    player_create_with(guid, &[])
}

/// [`player_create`] with `extra` fields.
fn player_create_with(guid: u64, extra: &[(u16, u32)]) -> SessionEvent {
    let mut pairs = vec![
        OBJECT_TYPE_PLAYER,
        (FIELD_UNIT_HEALTH, 500),
        (FIELD_UNIT_MAXHEALTH, 500),
        (FIELD_UNIT_LEVEL, 80),
    ];
    pairs.extend_from_slice(extra);
    SessionEvent::ObjectCreate {
        guid,
        kind: EntityKind::Player,
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
        fields: ObjectFields::from_pairs(&pairs).into_created(ObjectType::Player),
    }
}

/// A name answer for `guid`: a human warrior.
fn name_answer(guid: u64, name: &str) -> SessionEvent {
    SessionEvent::PlayerName {
        guid,
        name: name.into(),
        realm: String::new(),
        race: 1,
        class: 1,
        gender: 0,
        declensions: None,
    }
}

/// A finished one-row log for `guid`.
fn pvp_log(guid: u64) -> SessionEvent {
    SessionEvent::PvpLogData(benilla_protocol::messages::PvpLogData {
        ended: true,
        winner: Some(1),
        rows: vec![benilla_protocol::messages::PvpLogRow {
            guid,
            killing_blows: 5,
            ..Default::default()
        }],
        ..Default::default()
    })
}

/// A named log rebuilds in its handler (`0x54d495`), the stock filter call nesting its event (`0x54c120`), before a later status (`0x54b1a9`).
#[test]
fn a_log_and_a_status_in_one_frame_read_the_log_just_stored() {
    use benilla_protocol::messages::{BattlefieldActive, BattlefieldStatus};
    let Some(mut rig) = Rig::login(&[]) else {
        return;
    };
    let active = SessionEvent::BattlefieldStatus(BattlefieldStatus {
        slot: 0,
        queue: (2 << 16) | 2,
        status: 3,
        active: Some(BattlefieldActive {
            map_id: 489,
            joined_queue: 0,
            shutdown_ms: 0,
            run_ms: 30_000,
            arena_faction: 0,
        }),
        ..Default::default()
    });
    let seen = rig.packets(vec![name_answer(0x77, "Rival"), pvp_log(0x77), active], 2);
    assert_eq!(
        pin(
            &seen,
            &["UPDATE_BATTLEFIELD_STATUS", "UPDATE_BATTLEFIELD_SCORE"],
            &["scores", "winner", "score1", "bf1"]
        ),
        vec![
            "UPDATE_BATTLEFIELD_SCORE | scores=1 winner=1 score1=Rival bf1=none",
            "UPDATE_BATTLEFIELD_SCORE | scores=1 winner=1 score1=Rival bf1=none",
            "UPDATE_BATTLEFIELD_SCORE | scores=1 winner=1 score1=Rival bf1=active",
            "UPDATE_BATTLEFIELD_STATUS 1 | scores=1 winner=1 score1=Rival bf1=active",
        ]
    );
}

/// An uncached row holds the rebuild (`0x54aa39`) until the answer's callback runs it (`0x54ca30`).
#[test]
fn the_score_event_waits_for_the_last_name() {
    let Some(mut rig) = Rig::login(&[]) else {
        return;
    };
    let seen = rig.packets(vec![pvp_log(0x88)], 2);
    assert!(
        pin(&seen, &["UPDATE_BATTLEFIELD_SCORE"], &[]).is_empty(),
        "no rebuild with a name pending"
    );
    let now = rig.live_now();
    assert_eq!(
        now.line(&["scores", "winner", "score1"]),
        "NOW | scores=0 winner=1 score1=nil"
    );
    let seen = rig.packets(vec![name_answer(0x88, "Late")], 2);
    assert_eq!(
        pin(
            &seen,
            &["UPDATE_BATTLEFIELD_SCORE"],
            &["scores", "winner", "score1"]
        ),
        vec![
            "UPDATE_BATTLEFIELD_SCORE | scores=1 winner=1 score1=Late",
            "UPDATE_BATTLEFIELD_SCORE | scores=1 winner=1 score1=Late",
        ]
    );
}

/// Each `SMSG_BATTLEFIELD_LIST` fires its source's event (`0x54e390`); the stock close nests its `BATTLEFIELDS_CLOSED` (`0x549b55`).
#[test]
fn each_battlefield_list_fires_its_source_event_with_its_instances() {
    use benilla_protocol::messages::BattlefieldList;
    let Some(mut rig) = Rig::login(&[]) else {
        return;
    };
    let list = |battlemaster: u64, from_anywhere: bool, bg_type: u32, n: u32| {
        SessionEvent::BattlefieldList(BattlefieldList {
            battlemaster,
            from_anywhere,
            bg_type,
            instances: (1..=n).collect(),
            ..Default::default()
        })
    };
    const BM: u64 = 0xF130_0000_0000_4242;
    let seen = rig.packets(
        vec![
            list(BM, false, benilla_ui::script::RANDOM_BATTLEGROUND, 1),
            list(0, true, 2, 2),
            list(BM, false, 2, 3),
            list(0, false, 3, 4),
        ],
        2,
    );
    assert_eq!(
        pin(
            &seen,
            &[
                "NPC_PVPQUEUE_ANYWHERE",
                "PVPQUEUE_ANYWHERE_SHOW",
                "BATTLEFIELDS_SHOW",
                "BATTLEFIELDS_CLOSED"
            ],
            &["bfcount"]
        ),
        vec![
            "NPC_PVPQUEUE_ANYWHERE | bfcount=1",
            "PVPQUEUE_ANYWHERE_SHOW | bfcount=2",
            "BATTLEFIELDS_SHOW | bfcount=3",
            "BATTLEFIELDS_CLOSED | bfcount=4",
            "BATTLEFIELDS_CLOSED | bfcount=4",
            "BATTLEFIELDS_SHOW | bfcount=4",
        ]
    );
}

/// The battle-manager notices fire in wire order, each reading its own write (`0x54993d`, `0x549a15`).
#[test]
fn battle_manager_notices_fire_in_wire_order() {
    use benilla_protocol::messages::BattlefieldMgrNotice as N;
    let Some(mut rig) = Rig::login(&[]) else {
        return;
    };
    let seen = rig.packets(
        vec![
            N::QueueRequestResponse {
                queue_id: 7,
                area_id: 4197,
                accepted: true,
                logging_in: false,
                warmup: 0,
            },
            N::QueueInvite {
                queue_id: 7,
                warmup: 0,
            },
            N::EntryInvite {
                queue_id: 7,
                area_id: 4197,
                expire: 0,
            },
            N::Entered {
                queue_id: 7,
                invited: true,
                on_offense: false,
                cleared_afk: true,
            },
            N::StateChange { old: 1, new: 2 },
            N::EjectPending {
                queue_id: 7,
                remote: false,
            },
            N::Ejected {
                queue_id: 7,
                reason: 0,
                battle_status: 0,
                relocated: false,
            },
        ]
        .into_iter()
        .map(SessionEvent::BattlefieldMgr)
        .collect(),
        2,
    );
    let mgr: Vec<String> = seen
        .iter()
        .filter(|s| s.event.starts_with("BATTLEFIELD_MGR_"))
        .map(|s| s.line(&["wpvp"]))
        .collect();
    assert_eq!(
        mgr,
        vec![
            "BATTLEFIELD_MGR_QUEUE_REQUEST_RESPONSE 7 true false false false | wpvp=queued",
            "BATTLEFIELD_MGR_QUEUE_INVITE 7 false | wpvp=queued",
            "BATTLEFIELD_MGR_ENTRY_INVITE 7 | wpvp=confirm",
            "BATTLEFIELD_MGR_ENTERED 7 true false | wpvp=none",
            "BATTLEFIELD_MGR_STATE_CHANGE 1 2 | wpvp=none",
            "BATTLEFIELD_MGR_EJECT_PENDING 7 false | wpvp=none",
            "BATTLEFIELD_MGR_EJECTED 7 false false false false | wpvp=none",
        ]
    );
}

/// `DUEL_REQUESTED` only for a held player (`0x5cfd73`), `UNKNOWNOBJECT` while uncached (`0x5cfd7b`).
#[test]
fn a_held_challenger_raises_the_duel_with_its_name() {
    let Some(mut rig) = Rig::login(&[]) else {
        return;
    };
    rig.packets(vec![player_create(0x99), player_create(0x9a)], 2);
    let arbiter = 0xF110_0000_0000_0001;
    let seen = rig.packets(
        vec![
            name_answer(0x99, "Rival"),
            SessionEvent::DuelRequested {
                arbiter,
                challenger: 0x99,
            },
            SessionEvent::DuelComplete { started: false },
            SessionEvent::DuelRequested {
                arbiter,
                challenger: 0x9a,
            },
        ],
        2,
    );
    assert_eq!(
        pin(&seen, &["DUEL_REQUESTED", "DUEL_FINISHED"], &[]),
        vec![
            "DUEL_REQUESTED Rival",
            "DUEL_FINISHED",
            "DUEL_REQUESTED Unknown",
        ]
    );
}

/// A party-stats opponent signals from the handler (`0x54b840`); the world entry clears the slots (`0x54cbd9`), a reload's skips it (`0x54cb51`).
#[test]
fn arena_opponents_signal_from_the_handler_and_clear_at_world_entry() {
    let Some(mut rig) = Rig::login(&[]) else {
        return;
    };
    let seen = rig.packets(
        vec![SessionEvent::PartyMemberStats {
            guid: 0x55,
            full: true,
            info: Box::new(benilla_protocol::messages::PartyMemberStatsInfo {
                arena_opponent: true,
                ..Default::default()
            }),
        }],
        2,
    );
    assert_eq!(
        pin(&seen, &["ARENA_OPPONENT_UPDATE"], &["opps"]),
        vec![
            "ARENA_OPPONENT_UPDATE arena1 seen | opps=1",
            "ARENA_OPPONENT_UPDATE arena1 unseen | opps=1",
        ]
    );
    rig.app
        .world()
        .non_send_resource::<benilla_ui::script::UiScript>()
        .run("ReloadUI()")
        .unwrap();
    rig.run(4);
    let seen = rig.take();
    assert!(
        pin(&seen, &["PLAYER_ENTERING_WORLD"], &[]).len() == 1
            && pin(&seen, &["ARENA_OPPONENT_UPDATE"], &[]).is_empty(),
        "a reload's world entry keeps the slots"
    );
    assert_eq!(rig.live_now().line(&["opps"]), "NOW | opps=1");
}

/// A player of the other arena side, held, as a stream create inside an arena sends it.
fn opponent_create(guid: u64) -> SessionEvent {
    player_create_with(
        guid,
        &[(benilla_protocol::field::FIELD_PLAYER_BYTES_3, 1 << 24)],
    )
}

/// The party stats' arena byte for `guid` (`0x6cf9fa`).
fn arena_stats(guid: u64) -> SessionEvent {
    SessionEvent::PartyMemberStats {
        guid,
        full: true,
        info: Box::new(benilla_protocol::messages::PartyMemberStatsInfo {
            arena_opponent: true,
            ..Default::default()
        }),
    }
}

/// A rig inside Nagrand Arena (map 559) on side 0, with two opponents' names cached.
fn arena_rig() -> Option<Rig> {
    use benilla_protocol::messages::{BattlefieldActive, BattlefieldStatus};
    let mut rig = Rig::login(&[OBJECT_TYPE_PLAYER])?;
    rig.packets(
        vec![
            SessionEvent::BattlefieldStatus(BattlefieldStatus {
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
            }),
            name_answer(0x55, "Rival"),
            name_answer(0x56, "Other"),
        ],
        2,
    );
    Some(rig)
}

/// `arenaN` reads the slot just written at its event (`0x608180`), in wire order.
#[test]
fn an_arena_event_reads_its_token_at_the_event() {
    let Some(mut rig) = arena_rig() else {
        return;
    };
    let seen = rig.packets(vec![opponent_create(0x56), arena_stats(0x55)], 2);
    assert_eq!(
        pin(
            &seen,
            &["ARENA_OPPONENT_UPDATE"],
            &["opps", "a1name", "a2name"]
        ),
        vec![
            "ARENA_OPPONENT_UPDATE arena1 seen | opps=1 a1name=Other a2name=nil",
            "ARENA_OPPONENT_UPDATE arena2 seen | opps=2 a1name=Other a2name=nil",
            "ARENA_OPPONENT_UPDATE arena2 unseen | opps=2 a1name=Other a2name=nil",
        ]
    );
}

/// The other order: the stats name the opponent first, the create signals it again (`0x54d100`, `0x54b840`).
#[test]
fn an_arena_opponent_seen_before_it_streams_in_reads_its_unit_at_the_create() {
    let Some(mut rig) = arena_rig() else {
        return;
    };
    let seen = rig.packets(vec![arena_stats(0x56), opponent_create(0x56)], 2);
    assert_eq!(
        pin(&seen, &["ARENA_OPPONENT_UPDATE"], &["opps", "a1name"]),
        vec![
            "ARENA_OPPONENT_UPDATE arena1 seen | opps=1 a1name=nil",
            "ARENA_OPPONENT_UPDATE arena1 unseen | opps=1 a1name=nil",
            "ARENA_OPPONENT_UPDATE arena1 seen | opps=1 a1name=Other",
        ]
    );
    // Later in the frame and the next, the per-frame push agrees with the event's.
    assert_eq!(
        rig.live_now().line(&["opps", "a1name"]),
        "NOW | opps=1 a1name=Other"
    );
}

/// The party stats' opponent signals before its pet lands (`0x6cf9fa` -> `0x54b840`, then `0x6cfc45`).
#[test]
fn an_arena_opponents_pet_lands_after_the_opponents_signals() {
    let Some(mut rig) = arena_rig() else {
        return;
    };
    const ARENA_PET: u64 = 0xF140_0000_0200_0001;
    let pet = SessionEvent::ObjectCreate {
        guid: ARENA_PET,
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
            (2, 0x9),
            (FIELD_UNIT_HEALTH, 500),
            (FIELD_UNIT_MAXHEALTH, 500),
            (FIELD_UNIT_LEVEL, 80),
        ])
        .into_created(ObjectType::Unit),
    };
    let stats = SessionEvent::PartyMemberStats {
        guid: 0x55,
        full: true,
        info: Box::new(benilla_protocol::messages::PartyMemberStatsInfo {
            arena_opponent: true,
            pet_guid: Some(ARENA_PET),
            ..Default::default()
        }),
    };
    let seen = rig.packets(vec![pet, stats], 2);
    assert_eq!(
        pin(&seen, &["ARENA_OPPONENT_UPDATE"], &["opps", "ap1"]),
        vec![
            "ARENA_OPPONENT_UPDATE arena1 seen | opps=1 ap1=nil",
            "ARENA_OPPONENT_UPDATE arena1 unseen | opps=1 ap1=nil",
            "ARENA_OPPONENT_UPDATE arenapet1 seen | opps=1 ap1=1",
        ]
    );
}

/// Our honor watches fire in field order, each reading its update (`0x6e5671`, `0x6e55fc`, `0x6da3c0`).
#[test]
fn honor_watches_fire_in_field_order_with_the_values_just_applied() {
    use benilla_protocol::field::{
        FIELD_PLAYER_BYTES_3, FIELD_PLAYER_FIELD_HONOR_CURRENCY, FIELD_PLAYER_FIELD_KILLS,
    };
    let Some(mut rig) = Rig::login(&[
        OBJECT_TYPE_PLAYER,
        (FIELD_PLAYER_FIELD_KILLS, 2),
        (FIELD_PLAYER_FIELD_HONOR_CURRENCY, 100),
    ]) else {
        return;
    };
    assert_eq!(
        rig.live_now().line(&["kills", "honorpts"]),
        "NOW | kills=2 honorpts=100"
    );
    let events = [
        "PLAYER_PVP_RANK_CHANGED",
        "PLAYER_PVP_KILLS_CHANGED",
        "HONOR_CURRENCY_UPDATE",
        "CURRENCY_DISPLAY_UPDATE",
    ];
    let seen = rig.packets(
        vec![self_values(&[
            (FIELD_PLAYER_FIELD_HONOR_CURRENCY, 150),
            (FIELD_PLAYER_FIELD_KILLS, 3),
            (FIELD_PLAYER_BYTES_3, 1 << 24),
        ])],
        2,
    );
    assert_eq!(
        pin(&seen, &events, &["kills", "honorpts"]),
        vec![
            "PLAYER_PVP_RANK_CHANGED player | kills=3 honorpts=150",
            "PLAYER_PVP_KILLS_CHANGED player | kills=3 honorpts=150",
            "HONOR_CURRENCY_UPDATE | kills=3 honorpts=150",
            "CURRENCY_DISPLAY_UPDATE | kills=3 honorpts=150",
        ]
    );
    let seen = rig.packets(
        vec![self_values(&[(FIELD_PLAYER_FIELD_KILLS, 3 | (9 << 16))])],
        2,
    );
    assert!(
        pin(&seen, &events, &[]).is_empty(),
        "yesterday's half has no watch"
    );
}

/// `GetPVPTimer` reads the stamp the flags watch set (`0x6e103a`).
#[test]
fn the_pvp_timer_reads_the_stamp_the_flags_watch_set() {
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER, (FIELD_PLAYER_FLAGS, 0x200)]) else {
        return;
    };
    assert_eq!(rig.live_now().line(&["pvptimer"]), "NOW | pvptimer=301000");
    rig.packets(vec![self_values(&[(FIELD_PLAYER_FLAGS, 0x40000)])], 1);
    let timer: i64 = rig
        .live_now()
        .verbs
        .get("pvptimer")
        .and_then(|v| v.parse().ok())
        .expect("a number");
    assert!(
        (290_000..=301_000).contains(&timer),
        "five minutes from the flag's edge: {timer}"
    );
}

/// The inspect target's honor reply is held and signalled from its handler (`0x5e7a50`); another's is dropped.
#[test]
fn the_inspect_honor_reply_signals_from_its_handler() {
    let Some(mut rig) = Rig::login(&[]) else {
        return;
    };
    rig.app
        .world()
        .resource::<crate::ui_inspect::InspectGuid>()
        .set(0x56);
    let reply = |guid| {
        SessionEvent::InspectHonorStats(benilla_protocol::messages::InspectHonorStats {
            player_guid: guid,
            highest_rank: 0,
            today_hk: 4,
            yesterday_hk: 0,
            today_honor: 0,
            yesterday_honor: 0,
            lifetime_hk: 0,
        })
    };
    let seen = rig.packets(vec![reply(0x99), reply(0x56)], 2);
    assert_eq!(
        pin(&seen, &["INSPECT_HONOR_UPDATE"], &["inspecthonor"]),
        vec!["INSPECT_HONOR_UPDATE | inspecthonor=1"]
    );
}

/// Arena-team events fire from their handlers in wire order (`0x5a28c0`, `0x5a37a2`, `0x5a2d50`, `0x5a3e10`).
#[test]
fn arena_team_events_fire_from_their_handlers_in_wire_order() {
    use benilla_protocol::field::FIELD_PLAYER_FIELD_ARENA_TEAM_INFO_1_1 as INFO;
    use benilla_protocol::messages::{
        ArenaTeamMember, ArenaTeamRecord, ArenaTeamRoster, ArenaTeamStats,
    };
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    let seen = rig.packets(
        vec![
            SessionEvent::ArenaTeamQueryResponse(ArenaTeamRecord {
                team_id: 9,
                name: "Probes".into(),
                team_type: 2,
                ..Default::default()
            }),
            self_values(&[(INFO, 9), (INFO + 1, 2), (INFO + 6, 1500)]),
            SessionEvent::ArenaTeamStats(ArenaTeamStats {
                team_id: 9,
                rating: 1520,
                ..Default::default()
            }),
            SessionEvent::ArenaTeamRoster(ArenaTeamRoster {
                team_id: 9,
                team_type: 2,
                members: vec![ArenaTeamMember {
                    guid: 0x56,
                    name: "Other".into(),
                    ..Default::default()
                }],
            }),
        ],
        2,
    );
    assert_eq!(
        pin(
            &seen,
            &["ARENA_TEAM_UPDATE", "ARENA_TEAM_ROSTER_UPDATE"],
            &["inteam", "team1", "rating1", "members1"]
        ),
        vec![
            "ARENA_TEAM_UPDATE | inteam=false team1=nil rating1=0 members1=0",
            "ARENA_TEAM_UPDATE | inteam=true team1=Probes rating1=0 members1=0",
            "ARENA_TEAM_UPDATE | inteam=true team1=Probes rating1=1520 members1=0",
            "ARENA_TEAM_ROSTER_UPDATE | inteam=true team1=Probes rating1=1520 members1=1",
        ]
    );
}

/// The inspect target's team reply writes its slot and signals from its handler (`0x5e7b57`, `0x5e7bdb`).
#[test]
fn the_inspect_arena_team_reply_signals_from_its_handler() {
    let Some(mut rig) = Rig::login(&[]) else {
        return;
    };
    rig.app
        .world()
        .resource::<crate::ui_inspect::InspectGuid>()
        .set(0x56);
    let team = |guid| {
        SessionEvent::InspectArenaTeam(benilla_protocol::messages::InspectArenaTeam {
            player_guid: guid,
            slot: 0,
            team_id: 9,
            rating: 1600,
            games: 0,
            wins: 0,
            played: 0,
            personal_rating: 0,
        })
    };
    let seen = rig.packets(
        vec![
            team(0x99),
            team(0x56),
            SessionEvent::ArenaTeamQueryResponse(benilla_protocol::messages::ArenaTeamRecord {
                team_id: 9,
                name: "Probes".into(),
                team_type: 2,
                ..Default::default()
            }),
        ],
        2,
    );
    assert_eq!(
        pin(
            &seen,
            &["INSPECT_HONOR_UPDATE", "ARENA_TEAM_UPDATE"],
            &["inspteam"]
        ),
        vec![
            "INSPECT_HONOR_UPDATE | inspteam=nil",
            "ARENA_TEAM_UPDATE | inspteam=Probes",
        ]
    );
}

/// A battleground status 3 on map `map` for slot 0, Warsong Gulch's queue word.
fn active_status(map: u32) -> SessionEvent {
    use benilla_protocol::messages::{BattlefieldActive, BattlefieldStatus};
    SessionEvent::BattlefieldStatus(BattlefieldStatus {
        slot: 0,
        queue: (2 << 16) | 2,
        status: 3,
        active: Some(BattlefieldActive {
            map_id: map,
            joined_queue: 0,
            shutdown_ms: 0,
            run_ms: 30_000,
            arena_faction: 0,
        }),
        ..Default::default()
    })
}

/// `SMSG_BATTLEFIELD_STATUS` for slot 0 with `status`, or the queue-0 clear with `None`.
fn slot0(status: Option<u32>) -> SessionEvent {
    use benilla_protocol::messages::BattlefieldStatus;
    SessionEvent::BattlefieldStatus(BattlefieldStatus {
        slot: 0,
        queue: status.map_or(0, |_| (2 << 16) | 2),
        status: status.unwrap_or(0),
        ..Default::default()
    })
}

/// `PLAYER_ENTERING_WORLD` reads the held status: the world entry zeroes after it (`0x5281a6`, then `0x528230` -> `0x54cb57`, `0x54cb5d`).
#[test]
fn player_entering_world_reads_the_held_status_on_a_reload() {
    let Some(mut rig) = Rig::login(&[]) else {
        return;
    };
    rig.packets(vec![slot0(Some(1))], 2);
    rig.app
        .world()
        .non_send_resource::<benilla_ui::script::UiScript>()
        .run("ReloadUI()")
        .unwrap();
    rig.run(4);
    let seen = rig.take();
    assert_eq!(
        pin(&seen, &["PLAYER_ENTERING_WORLD"], &["bf1"]),
        vec!["PLAYER_ENTERING_WORLD | bf1=queued"]
    );
    assert_eq!(rig.live_now().line(&["bf1"]), "NOW | bf1=none");
    let seen = rig.packets(vec![slot0(Some(1))], 2);
    assert_eq!(
        pin(&seen, &["UPDATE_BATTLEFIELD_STATUS"], &["bf1"]),
        vec!["UPDATE_BATTLEFIELD_STATUS 1 | bf1=queued"]
    );
}

/// The other order: a status ahead of our create at login is held at `PLAYER_ENTERING_WORLD`, then zeroed.
#[test]
fn player_entering_world_reads_a_status_from_before_the_login() {
    let Some(mut rig) = Rig::login_after(vec![slot0(Some(1))], &[]) else {
        return;
    };
    let seen = rig.take();
    assert_eq!(
        pin(&seen, &["PLAYER_ENTERING_WORLD"], &["bf1"]),
        vec!["PLAYER_ENTERING_WORLD | bf1=queued"]
    );
    assert_eq!(rig.live_now().line(&["bf1"]), "NOW | bf1=none");
}

/// `MSG_BATTLEGROUND_PLAYER_POSITIONS` naming `guids`, no carrier.
fn positions(guids: &[u64]) -> SessionEvent {
    use benilla_protocol::messages::{BattlefieldPosition, BattlefieldPositions};
    SessionEvent::BattlefieldPositions(BattlefieldPositions {
        players: guids
            .iter()
            .map(|&guid| BattlefieldPosition {
                guid,
                x: 1000.0,
                y: 1400.0,
            })
            .collect(),
        carriers: Vec::new(),
    })
}

/// A party list with `member` in our subgroup.
fn party_with(member: u64) -> SessionEvent {
    SessionEvent::GroupList {
        group_type: 0,
        own_flags: 0,
        own_roles: 0,
        members: vec![benilla_protocol::messages::GroupMemberEntry {
            name: "Mate".into(),
            guid: member,
            status: 1,
            flags: 0,
            roles: 0,
        }],
        leader: ME,
        loot: Some(benilla_protocol::messages::GroupLootInfo {
            method: 3,
            master: 0,
            threshold: 2,
        }),
        difficulties: None,
        lfg_restricted: false,
        lfg_status: 0,
        lfg_slot: 0,
        battleground: false,
        group_guid: 0,
        counter: 0,
    }
}

/// The position verbs read the list, group and names when called (`0x54c2e0`), one `WORLD_MAP_NAME_UPDATE` per ask (`0x5496c0`).
#[test]
fn battlefield_positions_read_the_group_and_names_when_called() {
    for party_first in [false, true] {
        let Some(mut rig) = Rig::login(&[]) else {
            return;
        };
        let pos = positions(&[0x50, 0x60, ME]);
        let frame = if party_first {
            vec![party_with(0x50), pos, active_status(489)]
        } else {
            vec![pos, active_status(489), party_with(0x50)]
        };
        let seen = rig.packets(frame, 2);
        let status = pin(&seen, &["UPDATE_BATTLEFIELD_STATUS"], &["bfpos"]);
        let want = if party_first { "1" } else { "2" };
        assert_eq!(
            status,
            vec![format!("UPDATE_BATTLEFIELD_STATUS 1 | bfpos={want}")],
            "party first: {party_first}"
        );
        assert_eq!(
            rig.live_now().line(&["bfpos", "bfpos1"]),
            "NOW | bfpos=1 bfpos1=nil",
            "the member drops out the moment its list lands"
        );
        let seen = rig.packets(vec![name_answer(0x60, "Rival")], 2);
        let updates = pin(&seen, &["WORLD_MAP_NAME_UPDATE"], &["bfpos1"]);
        assert!(!updates.is_empty(), "the answer runs the callbacks");
        assert!(
            updates
                .iter()
                .all(|l| l == "WORLD_MAP_NAME_UPDATE | bfpos1=Rival"),
            "{updates:?}"
        );
        let again = rig.packets(vec![name_answer(0x60, "Rival")], 2);
        assert!(
            pin(&again, &["WORLD_MAP_NAME_UPDATE"], &[]).is_empty(),
            "an answered ask is gone"
        );
    }
}

/// The active slot's queue-0 arm zeroes positions, columns and type before its event (`0x54b165`, `0x54b174`, `0x54b17e`).
#[test]
fn the_queue_zero_arm_clears_the_active_instance_before_its_event() {
    let Some(mut rig) = Rig::login(&[]) else {
        return;
    };
    let seen = rig.packets(vec![active_status(559), positions(&[0x60]), slot0(None)], 2);
    assert_eq!(
        pin(
            &seen,
            &["UPDATE_BATTLEFIELD_STATUS"],
            &["bf1", "arena", "bfpos", "bfstats"]
        ),
        vec![
            "UPDATE_BATTLEFIELD_STATUS 1 | bf1=active arena=1 bfpos=0 bfstats=0",
            "UPDATE_BATTLEFIELD_STATUS 1 | bf1=none arena=nil bfpos=0 bfstats=0",
        ]
    );
    let seen = rig.packets(
        vec![
            active_status(489),
            positions(&[0x60]),
            slot0(Some(1)),
            slot0(None),
        ],
        2,
    );
    let lines = pin(
        &seen,
        &["UPDATE_BATTLEFIELD_STATUS"],
        &["bf1", "arena", "bfpos", "bfstats"],
    );
    assert_eq!(lines.len(), 3, "{lines:?}");
    assert!(lines[0].starts_with("UPDATE_BATTLEFIELD_STATUS 1 | bf1=active arena=nil bfpos=0"));
    let columns = lines[0].rsplit_once(' ').unwrap().1;
    assert_ne!(columns, "bfstats=0", "Warsong Gulch has score columns");
    assert_eq!(
        lines[1],
        format!("UPDATE_BATTLEFIELD_STATUS 1 | bf1=queued arena=nil bfpos=1 {columns}")
    );
    assert_eq!(
        lines[2],
        format!("UPDATE_BATTLEFIELD_STATUS 1 | bf1=none arena=nil bfpos=1 {columns}"),
        "no active slot left for the arm"
    );
}
