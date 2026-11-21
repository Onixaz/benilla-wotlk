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
    { "critters", function() return GetNumCompanions("CRITTER") end },
    { "mount1", function() return (select(2, GetCompanionInfo("MOUNT", 1))) end },
    { "abandon", function() return PetCanBeAbandoned() end },
    { "happy", function() return (GetPetHappiness()) end },
    { "xp", function() local c, m = GetPetExperience(); return c .. "/" .. m end },
    { "critter1on", function() return (select(5, GetCompanionInfo("CRITTER", 1))) end },
    { "mount1cd", function() return (select(2, GetCompanionCooldown("MOUNT", 1))) end },
    { "possess", function() return IsPossessBarVisible() end },
    { "petattack", function() return IsPetAttackActive() end },
    { "petname", function() return (UnitName("pet")) end },
    { "petfollow", function() return (select(5, GetPetActionInfo(2))) end },
    { "petcd", function() return (GetPetActionCooldown(4)) > 0 end },
    { "petbar", function() return PetHasActionBar() end },
    { "petbook", function() return (HasPetSpells()) end },
    { "possname", function() return (select(2, GetPossessInfo(1))) end },
    { "popup", popup },
    { "button2", button2 },
    { "soulstone", function() return HasSoulstone() end },
    { "special", function() local l, t = GetQuestLogSpecialItemInfo(2) return l ~= nil and t ~= nil end },
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
    { "bf1", function() return (GetBattlefieldStatus(1)) end },
    { "bfrun", function() return GetBattlefieldInstanceRunTime() > 0 end },
    { "scores", function() return GetNumBattlefieldScores() end },
    { "winner", function() return GetBattlefieldWinner() end },
    { "score1", function() return (GetBattlefieldScore(1)) end },
    { "bfcount", function() return GetNumBattlefields() end },
    { "wpvp", function() return (GetWorldPVPQueueStatus(1)) end },
    { "opps", function() return GetNumArenaOpponents() end },
    { "p1", function() return UnitGUID("party1") end },
    { "veh", function() return UnitGUID("vehicle") end },
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
    { "wsui", function() return GetNumWorldStateUI() end },
    { "lb", function() return (GetQuestLogLeaderBoard(1, 2)) end },
    { "lbdone", function() return (select(3, GetQuestLogLeaderBoard(1, 2))) end },
    { "qtitle", function() return GetTitleText() end },
    { "qdone", function() return IsQuestCompletable() end },
    { "qreward", function() return GetRewardMoney() end },
    { "bankslots", function() return (GetNumBankSlots()) end },
    { "bankcost", function() return GetBankSlotCost() end },
    { "bankopen", function() return GetContainerFreeSlots(-1) ~= nil end },
    { "ws", function()
        local t = {}
        for i = 1, GetNumWorldStateUI() do t[#t + 1] = (select(3, GetWorldStateUIInfo(i))) end
        return table.concat(t, ",")
    end },
    { "atstable", function() return IsAtStableMaster() end },
    { "stabled", function() return GetNumStablePets() end },
    { "stablesel", function() return GetSelectedStablePet() end },
    { "rune2", function() return GetRuneType(2) end },
    { "rune2dur", function() return (select(2, GetRuneCooldown(2))) end },
    { "rune2ready", function() return (select(3, GetRuneCooldown(2))) end },
    { "totem1", function() return (GetTotemInfo(1)) end },
    { "totem1name", function() return (select(2, GetTotemInfo(1))) end },
    { "totem1left", function() return GetTotemTimeLeft(1) end },
    { "casting", function() return (UnitCastingInfo("player")) end },
    { "castid", function() return (select(8, UnitCastingInfo("player"))) end },
    { "stopcast", function() return SpellStopCasting ~= nil end },
    { "tabs", function() return GetNumSpellTabs() end },
    { "book1", function() return (GetSpellName(1, "spell")) end },
    { "book2", function() return (GetSpellName(2, "spell")) end },
    { "tip2", function()
        if not GetSpellName(2, "spell") then return nil end
        GameTooltip:SetOwner(UIParent, "ANCHOR_NONE")
        GameTooltip:SetSpell(2, "spell")
        local n, last = GameTooltip:NumLines(), GameTooltipTextLeft1:GetText()
        for i = 2, n do last = _G["GameTooltipTextLeft" .. i]:GetText() end
        GameTooltip:Hide()
        return n .. ":" .. tostring(last)
    end },
    { "greenrange", function() return GetQuestGreenRange() end },
    { "cursize", function() return GetCurrencyListSize() end },
    { "cur2", function() local n, _, _, _, _, c = GetCurrencyListInfo(2) return tostring(n) .. "x" .. tostring(c) end },
    { "free0", function() return (GetContainerNumFreeSlots(0)) end },
    { "cnt01", function() return (select(2, GetContainerItemInfo(0, 1))) end },
    { "lock01", function() return (select(3, GetContainerItemInfo(0, 1))) end },
    { "lock02", function() return (select(3, GetContainerItemInfo(0, 2))) end },
    { "lock05", function() return (select(3, GetContainerItemInfo(0, 5))) end },
    { "lock11", function() return (select(3, GetContainerItemInfo(1, 1))) end },
    { "lock12", function() return (select(3, GetContainerItemInfo(1, 2))) end },
    { "slots1", function() return GetContainerNumSlots(1) end },
    { "slots5", function() return GetContainerNumSlots(5) end },
    { "name5", function() return GetBagName(5) end },
    { "refund01", function() return (select(2, GetContainerItemPurchaseInfo(0, 1))) end },
    { "binderdist", function() return CheckBinderDist() end },
    { "talentdist", function() return CheckTalentMasterDist() end },
    { "boot", function() return GetInstanceBootTimeRemaining() end },
    { "spirittime", function() return GetAreaSpiritHealerTime() end },
    { "gbtabs", function() return GetNumGuildBankTabs() end },
    { "gbtab1", function() return (GetGuildBankTabInfo(1)) end },
    { "gbtab2", function() return (GetGuildBankTabInfo(2)) end },
    { "gbleft", function() return (select(6, GetGuildBankTabInfo(1))) end },
    { "gbcount4", function() return (select(2, GetGuildBankItemInfo(1, 4))) end },
    { "gbmoney", function() return GetGuildBankMoney() end },
    { "gbwithdraw", function() return GetGuildBankWithdrawMoney() end },
    { "gblog", function() return GetNumGuildBankTransactions(1) end },
    { "gblogname", function() return (select(2, GetGuildBankTransaction(1, 1))) end },
    { "gbtext", function() return GetGuildBankText(1) end },
    { "cansave", function() return TabardModel and TabardModel:CanSaveTabardNow() end },
    { "barberinfo", function() return select("#", GetBarberShopStyleInfo(2)) end },
    { "barbercost", function() return GetBarberShopTotalCost() end },
    { "gossiptext", function() return GetGossipText() end },
    { "gossipopts", function() return GetNumGossipOptions() end },
    { "ittext", function() return ItemTextGetText() end },
    { "itpage", function() return ItemTextGetPage() end },
    { "itnext", function() return ItemTextHasNextPage() end },
    { "merchantn", function() return GetMerchantNumItems() end },
    { "npc", function() return UnitGUID("npc") end },
    { "npcname", function() return UnitName("npc") end },
    { "buybackn", function() return GetNumBuybackItems() end },
    { "repaircost", function() return GetRepairAllCost() end },
    { "money", function() return GetMoney() end },
    { "trainn", function() return GetNumTrainerServices() end },
    { "trainsel", function() return GetTrainerSelectionIndex() end },
    { "taxin", function() return NumTaxiNodes() end },
    { "charter", function() return GetGuildCharterCost() end },
    { "petnames", function() return GetNumPetitionNames() end },
    { "pettitle", function() local _, t = GetPetitionInfo() return t end },
    { "petitems", function() return GetNumPetitionItems() end },
    { "taxicost", function()
        for i = 1, NumTaxiNodes() do
            if TaxiNodeName(i) == "Sentinel Hill, Westfall" then return TaxiNodeCost(i) end
        end
    end },
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
    pub(super) fn parse(line: &str) -> Self {
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
        Self::login_around(before, fields, Vec::new())
    }

    /// [`Self::login`] with `before` ahead of our create and `after` behind it, in the same frame.
    pub fn login_around(
        before: Vec<SessionEvent>,
        fields: &[(u16, u32)],
        after: Vec<SessionEvent>,
    ) -> Option<Self> {
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
        for ev in after {
            rig.send(ev);
        }
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
    // Read with the `World` lent, as a script call reads the boot deadline (`0x5162e0`).
    let clocks = |rig: &mut Rig| {
        super::live::with_live_world(rig.app.world_mut(), |s| {
            s.eval::<String>(
                "return GetInstanceBootTimeRemaining() .. ',' .. GetInstanceLockTimeRemaining()",
            )
        })
        .expect("a VM")
        .unwrap()
    };
    assert_eq!(clocks(&mut rig), "59,89", "the boot and lock clocks run");
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
    assert_eq!(clocks(&mut rig), "0,0");
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
        delay_ms: 120_500,
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
    // The slot watcher rebuilds the rows (`0x6df3bb`) and fires `QUEST_ACCEPTED` for a cached template at once (`0x6df49c`); the rebuild's `QUEST_LOG_UPDATE` waits for the frame's flush (`0x5e6cb9`, `0x614760`).
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

/// An accept whose template is not cached waits for its answer (`0x6df429`), its rebuild counting the miss and still signalling (`0x5e6a98`, `0x5e6cb9`); the answer rebuilds the rows first, its ask made first (`0x6df3bb`, `0x5de921`), so `QUEST_ACCEPTED` names the quest's row (`0x6df49c`).
#[test]
fn an_accept_waiting_on_its_template_names_the_rebuilt_row() {
    let Some(mut rig) = Rig::login(&[]) else {
        return;
    };
    let events = &["QUEST_LOG_UPDATE", "QUEST_ACCEPTED"];
    let verbs = &["quests"];
    let slot = rig.packets(vec![self_values(&[(FIELD_PLAYER_QUEST_LOG_1_1, QUEST)])], 2);
    let answer = rig.packets(vec![quest_template(0)], 3);
    assert_eq!(pin(&slot, events, verbs), ["QUEST_LOG_UPDATE | quests=0"]);
    assert_eq!(
        pin(&answer, events, verbs),
        vec!["QUEST_ACCEPTED 2 | quests=2", "QUEST_LOG_UPDATE | quests=2"]
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
    // Our summon field's watch fires `UNIT_PET` (`0x614190` slot 2), the token already our summon (`0x60acd0`); the packet's setter fires its own on the new guid (`0x5d6aca`), then the temporary bar's `PET_BAR_UPDATE` (`0x5d6ad2` -> `0x5d4c1b`).
    assert_eq!(
        pin(&summon, events, verbs),
        vec![
            "UNIT_PET player | pet=1 petui=nil",
            "UNIT_PET player | pet=1 petui=nil",
            "PET_BAR_UPDATE | pet=1 petui=nil",
        ]
    );
    assert_eq!(
        pin(&dismiss, events, verbs),
        vec![
            "UNIT_PET player | pet=nil petui=nil",
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

/// Accepting a quest whose source item is already carried: 12340's `GetQuestLogSpecialItemInfo` (`0x5e52d0`) reads the quest and the bags at call time, so the tracker's `QUEST_LOG_UPDATE` redraw already finds the special item.
#[test]
fn quest_accept_shows_the_special_item_with_the_log_update() {
    let Some((_, accept)) = accept_with_the_source_item(false) else {
        return;
    };
    let first = accept
        .iter()
        .find(|s| s.event == "QUEST_LOG_UPDATE")
        .expect("QUEST_LOG_UPDATE");
    assert_eq!(first.verbs["special"], "true");
}

/// The accept's `QUEST_ACCEPTED` (`0x6df49c`) and the flushed `QUEST_LOG_UPDATE` after it (`0x614760`) both read the special item: the verb reads the quest and the bags when called (`0x5e0da0`), as do the calls after the frames.
#[test]
fn quest_accept_with_a_carried_source_item() {
    let Some((mut rig, accept)) = accept_with_the_source_item(false) else {
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

/// The source item's create, its bag slot and the quest slot in one frame, as a server's accept sends them: `QUEST_ACCEPTED` and the flushed `QUEST_LOG_UPDATE` already read the item's link and art, which `0x5e52d0` builds from the instance at the call (`0x5e533d`, `0x5e536a`), no bag pass between.
#[test]
fn quest_accept_reads_a_source_item_created_in_the_same_frame() {
    let Some((mut rig, accept)) = accept_with_the_source_item(true) else {
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

/// A required item's create and destroy run the stack watch's walk (`0x5e1250`): the create's post-init with its record, at once with the record cached (`0x70af37` -> `0x70adfc`) or at the record's answer (`0x70ae90`), and the disable on a destroy (`0x70b0eb`); each on an item we own (`0x70ad20`-`0x70ad2f`, `0x70b0b5`-`0x70b0be`) queues `QUEST_LOG_UPDATE` and re-asks every giver's status (`0x5e12e5`). Another owner's item does neither.
#[test]
fn a_required_items_create_and_destroy_signal_the_log_and_reask_the_givers() {
    use benilla_protocol::field::{FIELD_ITEM_OWNER, FIELD_OBJECT_ENTRY};
    const ITEM: u32 = 5100;
    const LATE: u32 = 5101;
    let Some(mut rig) = Rig::login(&[]) else {
        return;
    };
    let mut t = benilla_protocol::messages::QuestTemplate {
        quest_id: QUEST,
        level: 80,
        zone_or_sort: 12,
        title: "Charz".into(),
        ..Default::default()
    };
    t.required_items[0] = (ITEM, 5);
    t.required_items[1] = (LATE, 1);
    rig.packets(
        vec![
            SessionEvent::QuestTemplate(Box::new(t)),
            self_values(&[(FIELD_PLAYER_QUEST_LOG_1_1, QUEST)]),
            SessionEvent::ItemTemplate {
                entry: ITEM,
                info: Some(Box::new(crate::items::test_template("Charz Pelt"))),
            },
        ],
        3,
    );
    let (tx, rx) = crossbeam_channel::unbounded();
    rig.app.insert_resource(crate::net::NetCommands(tx));
    let reasks = |rx: &crossbeam_channel::Receiver<crate::net::ClientCommand>| {
        rx.try_iter()
            .filter(|c| matches!(c, crate::net::ClientCommand::QuestgiverStatusMultipleQuery))
            .count()
    };
    let item = |guid: u64, entry: u32, owner: u64| SessionEvent::ItemCreate {
        guid,
        container: false,
        fields: ObjectFields::from_pairs(&[
            (FIELD_OBJECT_ENTRY, entry),
            (FIELD_ITEM_OWNER, owner as u32),
            (FIELD_ITEM_OWNER + 1, (owner >> 32) as u32),
        ])
        .into_created(ObjectType::Item),
    };
    let updates = |seen: &[Seen]| fired(seen, &["QUEST_LOG_UPDATE"]).len();

    let seen = rig.packets(vec![item(0x4000_0000_0000_0200, ITEM, ME)], 2);
    assert_eq!(
        updates(&seen),
        1,
        "a cached record: the post-init at the create"
    );
    assert_eq!(reasks(&rx), 1);

    let seen = rig.packets(vec![item(0x4000_0000_0000_0201, ITEM, 0x99)], 2);
    assert_eq!(updates(&seen), 0, "another owner's item");
    assert_eq!(reasks(&rx), 0);

    let seen = rig.packets(vec![item(0x4000_0000_0000_0202, LATE, ME)], 2);
    assert_eq!(updates(&seen), 0, "the record is asked for, not cached");
    assert_eq!(reasks(&rx), 0);
    let seen = rig.packets(
        vec![SessionEvent::ItemTemplate {
            entry: LATE,
            info: Some(Box::new(crate::items::test_template("Charz Tooth"))),
        }],
        2,
    );
    assert_eq!(updates(&seen), 1, "the record's answer runs the post-init");
    assert_eq!(reasks(&rx), 1);

    let seen = rig.packets(
        vec![SessionEvent::ObjectDestroyed(0x4000_0000_0000_0200)],
        2,
    );
    assert_eq!(updates(&seen), 1, "the destroy's disable");
    assert_eq!(reasks(&rx), 1);
    let seen = rig.packets(
        vec![SessionEvent::ObjectDestroyed(0x4000_0000_0000_0201)],
        2,
    );
    assert_eq!(updates(&seen), 0, "another owner's item");
    assert_eq!(reasks(&rx), 0);
}

/// Every write to one of our quest slots runs the slot watch (`0x6df370`): it re-asks every giver's status first (`0x6df3b4`), then after the rebuild and the accept an auto-complete quest (`0x6df4d6`) on a log row whose slot now passes the completion test (`0x6df4e2`) is completed toward the player itself (`0x6df505`). A kill counter short of its count, or a finished quest without the flag, sends no completion.
#[test]
fn the_slot_watch_reasks_then_auto_completes_a_finished_quest() {
    use crate::net::ClientCommand;
    use benilla_protocol::messages::{quest_flags, QuestTemplate, QUEST_LOG_SLOT_FIELDS};
    const PLAIN: u32 = QUEST + 1;
    let Some(mut rig) = Rig::login(&[]) else {
        return;
    };
    let template = |quest_id: u32, flags: u32| {
        let mut t = QuestTemplate {
            quest_id,
            level: 80,
            zone_or_sort: 12,
            title: "Charz".into(),
            flags,
            ..Default::default()
        };
        t.objectives[0].creature_or_go = 299;
        t.objectives[0].required_count = 2;
        SessionEvent::QuestTemplate(Box::new(t))
    };
    rig.packets(
        vec![
            template(QUEST, quest_flags::AUTO_COMPLETE),
            template(PLAIN, 0),
        ],
        2,
    );
    let (tx, rx) = crossbeam_channel::unbounded();
    rig.app.insert_resource(crate::net::NetCommands(tx));
    let sends = |rx: &crossbeam_channel::Receiver<ClientCommand>| {
        rx.try_iter()
            .filter_map(|c| match c {
                ClientCommand::QuestgiverStatusMultipleQuery => Some("reask".to_string()),
                ClientCommand::QuestgiverComplete { npc, quest } => {
                    Some(format!("complete {npc:#x} {quest}"))
                }
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    let counter = FIELD_PLAYER_QUEST_LOG_1_1 + 2;
    let plain = FIELD_PLAYER_QUEST_LOG_1_1 + QUEST_LOG_SLOT_FIELDS;

    rig.packets(vec![self_values(&[(FIELD_PLAYER_QUEST_LOG_1_1, QUEST)])], 2);
    assert_eq!(sends(&rx), ["reask"], "the accept, no kill yet");
    rig.packets(vec![self_values(&[(counter, 1)])], 2);
    assert_eq!(sends(&rx), ["reask"], "one kill of two");
    rig.packets(vec![self_values(&[(counter, 2)])], 2);
    assert_eq!(
        sends(&rx),
        ["reask".to_string(), format!("complete {ME:#x} {QUEST}")],
        "the second kill finishes it: the re-ask, then the completion toward ourselves"
    );
    rig.packets(vec![self_values(&[(plain, PLAIN), (plain + 2, 2)])], 2);
    assert_eq!(sends(&rx), ["reask"], "a finished quest without the flag");
}

/// Two POI responses in one frame: each handler writes the store in turn (`0x5e7370`) and queues `QUEST_POI_UPDATE` as a unique signal (`0x5e7628`), which fires once, at the frame's flush (`0x614760`), never per response.
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
        ui.write(move |s| {
            seen.lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(s.quest_poi_held(QUEST))
        });
    }
}

/// The POI response writes its store from the handler (`0x5e7370`), so the next packet's handler in the same frame already finds it; nothing waits for a per-frame pass.
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

/// `QUEST_POI_UPDATE` (`0x5e7628`) and the time response's `QUEST_LOG_UPDATE` (`0x5dff89`) are queued by their handlers and flushed in queue order (`0x614760`): wire order, either way round.
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

/// The quest-slot watch rebuilds the log (`0x6df3bb`), queueing `QUEST_LOG_UPDATE` (`0x5e6cb9`) from the update handler, so it keeps wire order with a POI response's `QUEST_POI_UPDATE` (`0x5e7628`) at the flush (`0x614760`).
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

/// The time response sets the offset and the reset (`0x5dff40`) and queues `QUEST_LOG_UPDATE` (`0x5dff89`); `GetQuestResetTime` reads them when called (`0x5e6de0`), 0 before any response.
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
fn accept_with_the_source_item(same_frame: bool) -> Option<(Rig, Vec<Seen>)> {
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
    let item = vec![
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
    ];
    let quest = vec![
        quest_template(ITEM),
        self_values(&[(FIELD_PLAYER_QUEST_LOG_1_1, QUEST)]),
    ];
    if same_frame {
        let accept = rig.packets(item.into_iter().chain(quest).collect(), 3);
        return Some((rig, accept));
    }
    rig.packets(item, 3);
    let accept = rig.packets(quest, 3);
    Some((rig, accept))
}

/// The kill quest the progress pins accept: one creature objective, five to kill.
fn kill_quest_template() -> SessionEvent {
    let mut t = benilla_protocol::messages::QuestTemplate {
        quest_id: QUEST,
        level: 80,
        zone_or_sort: 12,
        title: "Charz".into(),
        ..Default::default()
    };
    t.objectives[0] = benilla_protocol::messages::QuestObjective {
        creature_or_go: 299,
        required_count: 5,
        ..Default::default()
    };
    SessionEvent::QuestTemplate(Box::new(t))
}

/// The kill's two packets: the slot's counter word (`PLAYER_QUEST_LOG_1_3`) and the add-kill line.
fn kill(count: u32) -> [SessionEvent; 2] {
    [
        self_values(&[(FIELD_PLAYER_QUEST_LOG_1_1 + 2, count)]),
        SessionEvent::QuestObjectiveKill {
            quest_id: QUEST,
            entry: 299,
            count,
            required: 5,
        },
    ]
}

const PROGRESS_EVENTS: &[&str] = &[
    "UNIT_QUEST_LOG_CHANGED",
    "QUEST_WATCH_UPDATE",
    "QUEST_LOG_UPDATE",
    "UI_INFO_MESSAGE",
];

/// A kill's progress, the counter word and the add-kill line in either wire order: what fires and what the leaderboard reads at each event, pinned ahead of the progress move (#1276).
#[test]
fn quest_progress_in_both_packet_orders() {
    let Some(mut rig) = Rig::login(&[]) else {
        return;
    };
    rig.packets(
        vec![
            // The add-kill handler names the creature from its cached record (`0x6d82bc`).
            SessionEvent::CreatureName {
                entry: 299,
                name: Some("Wolf".into()),
                subname: None,
                cursor_name: None,
                creature_type: Some(1),
                pet_family: 0,
                rank: 0,
                type_flags: 0,
                display_id: 0,
                civilian: false,
                racial_leader: false,
                modifiers: (1.0, 1.0),
                movement_id: 0,
            },
            kill_quest_template(),
            self_values(&[(FIELD_PLAYER_QUEST_LOG_1_1, QUEST)]),
        ],
        3,
    );
    let [values, line] = kill(1);
    let values_first = rig.packets(vec![values, line], 3);
    let [values, line] = kill(2);
    let line_first = rig.packets(vec![line, values], 3);
    assert_eq!(
        (
            pin(&values_first, PROGRESS_EVENTS, &["lb", "lbdone"]),
            pin(&line_first, PROGRESS_EVENTS, &["lb", "lbdone"]),
            fired(&values_first, &["UI_ERROR_MESSAGE", "CHAT_MSG_SYSTEM"]),
        ),
        (
            vec![
                // The counter word fires the log window's watch (`0x60c280`); the add-kill handler's line and watch event (`0x6d82e9`, `0x6d8241`) read the slot at the call (`0x5e1460`).
                "UNIT_QUEST_LOG_CHANGED player | lb=Wolf slain: 1/5 lbdone=nil",
                "UI_INFO_MESSAGE Wolf slain: 1/5 | lb=Wolf slain: 1/5 lbdone=nil",
                "QUEST_WATCH_UPDATE 2 | lb=Wolf slain: 1/5 lbdone=nil",
                "QUEST_LOG_UPDATE | lb=Wolf slain: 1/5 lbdone=nil",
            ]
            .into_iter()
            .map(String::from)
            .collect::<Vec<_>>(),
            vec![
                "UI_INFO_MESSAGE Wolf slain: 2/5 | lb=Wolf slain: 1/5 lbdone=nil",
                "QUEST_WATCH_UPDATE 2 | lb=Wolf slain: 1/5 lbdone=nil",
                "UNIT_QUEST_LOG_CHANGED player | lb=Wolf slain: 2/5 lbdone=nil",
                "QUEST_LOG_UPDATE | lb=Wolf slain: 2/5 lbdone=nil",
            ]
            .into_iter()
            .map(String::from)
            .collect::<Vec<_>>(),
            Vec::<String>::new(),
        )
    );
}

const DIALOG_EVENTS: &[&str] = &[
    "QUEST_DETAIL",
    "QUEST_PROGRESS",
    "QUEST_COMPLETE",
    "QUEST_FINISHED",
    "QUEST_ITEM_UPDATE",
    "QUEST_ACCEPT_CONFIRM",
];

/// The quest dialogs as their packets arrive one frame apart and two in one frame, and the escort confirm: what fires and what the dialog verbs read at each event, pinned ahead of the dialog move (#1276).
#[test]
fn quest_dialogs_and_the_escort_confirm() {
    use benilla_protocol::messages::{
        QuestConfirmAccept, QuestDetails, QuestOfferReward, QuestRequestItems,
    };
    let Some(mut rig) = Rig::login(&[]) else {
        return;
    };
    const NPC: u64 = 0xF130_0000_0100_0001;
    let detail = || {
        SessionEvent::QuestDetail(QuestDetails {
            npc: NPC,
            quest_id: QUEST,
            title: "Charz".into(),
            details: "Go.".into(),
            objectives: "Kill.".into(),
            ..Default::default()
        })
    };
    let progress = || {
        SessionEvent::QuestProgress(QuestRequestItems {
            npc: NPC,
            quest_id: QUEST,
            title: "Charz".into(),
            request_text: "Well?".into(),
            is_complete: true,
            ..Default::default()
        })
    };
    let offer = || {
        SessionEvent::QuestOffer(QuestOfferReward {
            npc: NPC,
            quest_id: QUEST,
            title: "Charz".into(),
            offer_text: "Thanks.".into(),
            ..Default::default()
        })
    };
    let verbs = &["qtitle", "qdone"];
    let mut seen = Vec::new();
    for ev in [detail(), progress(), offer()] {
        seen.push(pin(&rig.packets(vec![ev], 2), DIALOG_EVENTS, verbs));
    }
    seen.push(pin(
        &rig.packets(
            vec![SessionEvent::QuestComplete(
                benilla_protocol::messages::QuestComplete {
                    quest_id: QUEST,
                    xp: 100,
                    ..Default::default()
                },
            )],
            2,
        ),
        DIALOG_EVENTS,
        verbs,
    ));
    seen.push(pin(
        &rig.packets(vec![detail(), progress()], 2),
        DIALOG_EVENTS,
        verbs,
    ));
    seen.push(pin(
        &rig.packets(
            vec![
                name_answer(0x99, "Escort"),
                SessionEvent::QuestConfirmAccept(QuestConfirmAccept {
                    quest_id: QUEST,
                    title: "Charz".into(),
                    sender: 0x99,
                }),
            ],
            2,
        ),
        DIALOG_EVENTS,
        &[],
    ));
    // No NPC is in view, so no panel takes the interact target (`0x58c5c7`) and nothing closes one: no `QUEST_FINISHED`.
    let lines = |l: &[&str]| l.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert_eq!(
        seen,
        vec![
            lines(&["QUEST_DETAIL | qtitle=Charz qdone=nil"]),
            lines(&["QUEST_PROGRESS | qtitle=Charz qdone=1"]),
            lines(&["QUEST_COMPLETE | qtitle=Charz qdone=nil"]),
            // The template is not cached: the turn-in waits for its answer (`0x6d1192`).
            lines(&[]),
            // Each handler fires its panel's event in its turn (`0x58dfb5`, then `0x58bb4c`).
            lines(&[
                "QUEST_DETAIL | qtitle=Charz qdone=nil",
                "QUEST_PROGRESS | qtitle=Charz qdone=1",
            ]),
            lines(&["QUEST_ACCEPT_CONFIRM Escort Charz"]),
        ]
    );
}

/// A dialog and a quest-slot update in one frame, in either wire order: the dialog's event fires before the frame's `QUEST_LOG_UPDATE`, which waits for the idle tail after every handler (`0x52b247` -> `0x614760`).
#[test]
fn a_dialog_and_a_slot_update_in_one_frame_fire_the_log_update_last() {
    use benilla_protocol::messages::QuestDetails;
    let Some(mut rig) = Rig::login(&[]) else {
        return;
    };
    let detail = || {
        SessionEvent::QuestDetail(QuestDetails {
            npc: 0xF130_0000_0100_0001,
            quest_id: QUEST,
            title: "Charz".into(),
            ..Default::default()
        })
    };
    let events = &["QUEST_DETAIL", "QUEST_LOG_UPDATE"];
    rig.packets(vec![quest_template(0)], 3);
    let dialog_first = rig.packets(
        vec![
            detail(),
            self_values(&[(FIELD_PLAYER_QUEST_LOG_1_1, QUEST)]),
        ],
        1,
    );
    let slot_first = rig.packets(
        vec![self_values(&[(FIELD_PLAYER_QUEST_LOG_1_1, 0)]), detail()],
        1,
    );
    assert_eq!(
        fired(&dialog_first, events),
        ["QUEST_DETAIL", "QUEST_LOG_UPDATE"]
    );
    assert_eq!(
        fired(&slot_first, events),
        ["QUEST_DETAIL", "QUEST_LOG_UPDATE"]
    );
}

/// The events the progress, completed-quest and escort handlers fire, beside the control handler's, in one frame.
const TURN_EVENTS: &[&str] = &[
    "UI_INFO_MESSAGE",
    "QUEST_WATCH_UPDATE",
    "QUEST_QUERY_COMPLETE",
    "QUEST_ACCEPT_CONFIRM",
    "PLAYER_CONTROL_LOST",
    "PLAYER_CONTROL_GAINED",
];

/// The add-kill line and `QUEST_WATCH_UPDATE` (`0x6d82e9`, `0x6d8241`), `QUEST_QUERY_COMPLETE` (`0x5b5270`) and `QUEST_ACCEPT_CONFIRM` (`0x58bc50`) fire in their packet's turn: ahead of a later packet's handler event in the same frame, behind an earlier one's.
#[test]
fn progress_completed_and_confirm_fire_in_their_packets_turn() {
    use benilla_protocol::messages::QuestConfirmAccept;
    let Some(mut rig) = Rig::login(&[]) else {
        return;
    };
    rig.packets(
        vec![
            name_answer(0x99, "Escort"),
            SessionEvent::CreatureName {
                entry: 299,
                name: Some("Wolf".into()),
                subname: None,
                cursor_name: None,
                creature_type: Some(1),
                pet_family: 0,
                rank: 0,
                type_flags: 0,
                display_id: 0,
                civilian: false,
                racial_leader: false,
                modifiers: (1.0, 1.0),
                movement_id: 0,
            },
            kill_quest_template(),
            self_values(&[(FIELD_PLAYER_QUEST_LOG_1_1, QUEST)]),
        ],
        3,
    );
    let [counter, line] = kill(1);
    rig.packets(vec![counter], 2);
    let line_first = rig.packets(vec![line, control(false)], 1);
    let [counter, line] = kill(2);
    rig.packets(vec![counter], 2);
    let control_first = rig.packets(vec![control(true), line], 1);
    let completed = || SessionEvent::QuestsCompleted(vec![QUEST]);
    let confirm = |sender| {
        SessionEvent::QuestConfirmAccept(QuestConfirmAccept {
            quest_id: QUEST,
            title: "Charz".into(),
            sender,
        })
    };
    let answers_first = rig.packets(vec![completed(), confirm(0x99), control(false)], 1);
    let control_ahead = rig.packets(vec![control(true), completed(), confirm(0x99)], 1);
    assert_eq!(
        (
            fired(&line_first, TURN_EVENTS),
            fired(&control_first, TURN_EVENTS),
            fired(&answers_first, TURN_EVENTS),
            fired(&control_ahead, TURN_EVENTS),
        ),
        (
            vec![
                "UI_INFO_MESSAGE".to_string(),
                "QUEST_WATCH_UPDATE".into(),
                "PLAYER_CONTROL_LOST".into(),
            ],
            vec![
                "PLAYER_CONTROL_GAINED".to_string(),
                "UI_INFO_MESSAGE".into(),
                "QUEST_WATCH_UPDATE".into(),
            ],
            vec![
                "QUEST_QUERY_COMPLETE".to_string(),
                "QUEST_ACCEPT_CONFIRM".into(),
                "PLAYER_CONTROL_LOST".into(),
            ],
            vec![
                "PLAYER_CONTROL_GAINED".to_string(),
                "QUEST_QUERY_COMPLETE".into(),
                "QUEST_ACCEPT_CONFIRM".into(),
            ],
        )
    );
}

/// The escort confirm and the push verdict read the member's name with no callback (`0x58bc85`, `0x6e3a61`): a name not cached drops them, and its answer later in the frame raises nothing; a cached name shows the verdict's line.
#[test]
fn an_unnamed_member_drops_the_confirm_and_the_verdict() {
    use benilla_protocol::messages::{QuestConfirmAccept, QuestShareMsg};
    let Some(mut rig) = Rig::login(&[]) else {
        return;
    };
    let confirm = SessionEvent::QuestConfirmAccept(QuestConfirmAccept {
        quest_id: QUEST,
        title: "Charz".into(),
        sender: 0x77,
    });
    let verdict = |member| SessionEvent::QuestPushResult {
        member,
        msg: QuestShareMsg::SHARING_QUEST,
    };
    let unnamed = rig.packets(vec![confirm, verdict(0x77), name_answer(0x77, "Late")], 3);
    let named = rig.packets(vec![verdict(0x77)], 3);
    let events = &["QUEST_ACCEPT_CONFIRM", "CHAT_MSG_SYSTEM"];
    assert_eq!(fired(&unnamed, events), Vec::<String>::new());
    let lines: Vec<&str> = named
        .iter()
        .filter(|s| s.event == "CHAT_MSG_SYSTEM")
        .map(|s| s.args.split(' ').take(4).collect::<Vec<_>>().join(" "))
        .map(|l| {
            if l.starts_with("Sharing quest with Late") {
                "shared"
            } else {
                "other"
            }
        })
        .collect();
    assert_eq!(lines, ["shared"], "{named:?}");
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
    // handler registered before it and the flags packet's `UNIT_FLAGS` (`0x60c240`) ahead of both;
    // the stock handler ends in the usable update (`0x52102a`).
    assert_eq!(
        pin(&ahead, &["CHARZ_HANDLER"], &["taxi"]),
        vec!["CHARZ_HANDLER lost | taxi=1"]
    );
    let first: Vec<&str> = ahead.iter().take(4).map(|s| s.event.as_str()).collect();
    assert_eq!(
        first,
        [
            "UNIT_FLAGS",
            "PLAYER_CONTROL_LOST",
            "SPELL_UPDATE_USABLE",
            "CHARZ_HANDLER"
        ]
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
        offerer: None,
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

/// The world-state handler fires `ARENA_SEASON_WORLD_STATE` from the setter (`0x5494db`), refreshes the map (`0x5269c5`) and fires `UPDATE_WORLD_STATES` (`0x5269d1`, `0x526a17`) before the next packet's handler runs, and a moved phase fires it too (`0x51397f`); the readout reads the values each handler just wrote.
#[test]
fn world_state_events_fire_from_their_handlers_in_wire_order() {
    let Some(mut rig) = Rig::login(&[]) else {
        return;
    };
    let seen = rig.packets(
        vec![
            SessionEvent::WorldStates {
                scope: Some((489, 3277, 3277)),
                states: vec![(1581, 2), (0xc77, 8)],
            },
            SessionEvent::DuelOutOfBounds,
            SessionEvent::PhaseShift { mask: 0x41 },
            SessionEvent::PhaseShift { mask: 0x41 },
            SessionEvent::WorldStates {
                scope: None,
                states: vec![(1581, 3), (0xc77, 8)],
            },
        ],
        2,
    );
    assert_eq!(
        pin(
            &seen,
            &[
                "ARENA_SEASON_WORLD_STATE",
                "UPDATE_WORLD_STATES",
                "DUEL_OUTOFBOUNDS",
                "WORLD_MAP_UPDATE",
            ],
            &["ws"]
        ),
        vec![
            "ARENA_SEASON_WORLD_STATE | ws=Time Remaining: 0 min.,2/0,0/0",
            "WORLD_MAP_UPDATE | ws=Time Remaining: 0 min.,2/0,0/0",
            "UPDATE_WORLD_STATES | ws=Time Remaining: 0 min.,2/0,0/0",
            "DUEL_OUTOFBOUNDS | ws=Time Remaining: 0 min.,2/0,0/0",
            "UPDATE_WORLD_STATES | ws=Time Remaining: 0 min.,2/0,0/0",
            "WORLD_MAP_UPDATE | ws=Time Remaining: 0 min.,3/0,0/0",
            "UPDATE_WORLD_STATES | ws=Time Remaining: 0 min.,3/0,0/0",
        ],
        "the second phase shift moves nothing and the second season write moves nothing"
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

/// Each `SMSG_BATTLEFIELD_LIST` fires its source's event (`0x54e390`); the stock close nests its `BATTLEFIELDS_CLOSED` (`0x549b55`). The anywhere list's battlemaster holds the interact target, so the same battlemaster's list re-sets it and the clear's walk closes that battlemaster first (`0x54970f` -> `0x51fd1c` -> `0x51309f` -> `0x5496dc`).
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
            "BATTLEFIELDS_CLOSED | bfcount=2",
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

/// `CloseQuest` and `AcceptQuest` close inside their call (`0x58d35a` -> `0x58ca70`, `0x58cdc5`): the rest of the chunk already sees `QUEST_FINISHED` fired, the giver and its `"questnpc"` gone and the panel's text kept (`0x58cb6b`, `0x58cb98`, `0x58cded`).
#[test]
fn close_and_accept_finish_the_window_inside_their_call() {
    use benilla_protocol::messages::QuestDetails;
    let Some(mut rig) = Rig::login(&[]) else {
        return;
    };
    const NPC: u64 = 0xF130_0000_0100_0001;
    let detail = || {
        SessionEvent::QuestDetail(QuestDetails {
            npc: NPC,
            quest_id: QUEST,
            title: "Charz".into(),
            ..Default::default()
        })
    };
    let chunk = |verb: &str| {
        format!(
            "local before = #(CHARZ_LOG or {{}}) {verb}() local fired = 0 \
             for i = before + 1, #(CHARZ_LOG or {{}}) do \
               if CHARZ_LOG[i]:find('QUEST_FINISHED', 1, true) then fired = fired + 1 end end \
             return fired .. '|' .. tostring(GetTitleText()) .. '|' .. tostring(UnitExists('questnpc'))"
        )
    };
    // The giver held beside us, so the window's range guard keeps it open.
    rig.packets(vec![npc_create(NPC, 0x2)], 1);
    for verb in ["CloseQuest", "AcceptQuest"] {
        let opened = rig.packets(vec![detail()], 1);
        assert_eq!(
            fired(&opened, &["QUEST_DETAIL", "QUEST_FINISHED"]),
            ["QUEST_DETAIL"],
            "{verb}: the window is open"
        );
        assert!(rig
            .app
            .world()
            .non_send_resource::<crate::ui_quest::QuestGiver>()
            .is_open());
        let read =
            super::live::with_live_world(rig.app.world_mut(), |s| s.eval::<String>(&chunk(verb)))
                .expect("a VM")
                .expect("the chunk runs");
        assert_eq!(read, "1|Charz|nil", "{verb}");
        assert!(!rig
            .app
            .world()
            .non_send_resource::<crate::ui_quest::QuestGiver>()
            .is_open());
        // The next frames drain the log: the call's own `QUEST_FINISHED`, and no second one.
        rig.run(2);
        assert_eq!(
            fired(&rig.take(), &["QUEST_FINISHED"]),
            ["QUEST_FINISHED"],
            "{verb}"
        );
    }
}

/// The banker's guid.
const BANKER: u64 = 0xF130_0000_0200_0001;

/// The banker as a stream create sends it, with `npc_flags`.
fn banker_create(npc_flags: u32) -> SessionEvent {
    npc_create(BANKER, npc_flags)
}

/// `UNIT_NPC_FLAG_GOSSIP`: a flag the interact target's watchdog wants to keep a unit held (`0x7295cc`).
pub(super) const GOSSIP_NPC: u32 = crate::target::cursor_mode::npc_flags::GOSSIP;

/// A creature as a stream create sends it, with `npc_flags`.
pub(super) fn npc_create(guid: u64, npc_flags: u32) -> SessionEvent {
    let pairs = vec![
        (2, 0x9),
        (FIELD_UNIT_HEALTH, 500),
        (FIELD_UNIT_MAXHEALTH, 500),
        (FIELD_UNIT_LEVEL, 30),
        (benilla_protocol::field::FIELD_UNIT_NPC_FLAGS, npc_flags),
    ];
    SessionEvent::ObjectCreate {
        guid,
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
        fields: ObjectFields::from_pairs(&pairs).into_created(ObjectType::Unit),
    }
}

/// Our purchased bank bag slots, `PLAYER_BYTES_2` byte 2.
fn bank_slots(n: u32) -> SessionEvent {
    self_values(&[(benilla_protocol::field::FIELD_PLAYER_BYTES_2, n << 16)])
}

const BANK_EVENTS: &[&str] = &[
    "BANKFRAME_OPENED",
    "BANKFRAME_CLOSED",
    "PLAYERBANKBAGSLOTS_CHANGED",
    "UI_ERROR_MESSAGE",
];

/// `SMSG_SHOW_BANK` sets the banker and fires `BANKFRAME_OPENED` in its handler (`0x6e352a` -> `0x57b926`); the slot byte's watch fires `PLAYERBANKBAGSLOTS_CHANGED` whether a banker is open or not, its count read live (`0x57b8b0`, `0x57bab0`); a buy refusal shows in its handler's turn (`0x6e355d`).
#[test]
fn bank_open_slot_byte_and_refusal_fire_from_their_handlers() {
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    let verbs = &["bankslots", "bankopen"];
    // The byte moves with no bank open: the watch fires all the same.
    let seen = rig.packets(vec![bank_slots(1)], 2);
    assert_eq!(
        pin(&seen, BANK_EVENTS, verbs),
        ["PLAYERBANKBAGSLOTS_CHANGED | bankslots=1 bankopen=false"]
    );
    let seen = rig.packets(
        vec![
            banker_create(0x20000),
            SessionEvent::ShowBank { banker: BANKER },
            bank_slots(2),
            SessionEvent::BuyBankSlotResult { result: 1 },
        ],
        2,
    );
    assert_eq!(
        pin(&seen, BANK_EVENTS, verbs),
        [
            "BANKFRAME_OPENED | bankslots=1 bankopen=true",
            "PLAYERBANKBAGSLOTS_CHANGED | bankslots=2 bankopen=true",
            "UI_ERROR_MESSAGE You can't afford that. | bankslots=2 bankopen=true",
        ]
    );
    // The other packet order: the byte first, then the bank.
    let seen = rig.packets(
        vec![bank_slots(3), SessionEvent::ShowBank { banker: BANKER }],
        2,
    );
    assert_eq!(
        pin(&seen, BANK_EVENTS, verbs),
        [
            "PLAYERBANKBAGSLOTS_CHANGED | bankslots=3 bankopen=true",
            "BANKFRAME_CLOSED | bankslots=3 bankopen=true",
            "BANKFRAME_CLOSED | bankslots=3 bankopen=false",
            "BANKFRAME_OPENED | bankslots=3 bankopen=true",
        ],
        "a second banker over an open bank closes it through the interaction close first; the stock frame's OnHide calls CloseBankFrame inside that fire, ahead of the observer"
    );
}

/// `CloseBankFrame` fires `BANKFRAME_CLOSED` with the banker still set, then again from the interaction close's bank leg, then clears it, all inside its call (`0x57b9c0` -> `0x57b8d0`, `0x512fa7`).
#[test]
fn close_bank_frame_closes_inside_its_call() {
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    let seen = rig.packets(
        vec![
            banker_create(0x20000),
            SessionEvent::ShowBank { banker: BANKER },
        ],
        2,
    );
    assert_eq!(fired(&seen, &["BANKFRAME_OPENED"]), ["BANKFRAME_OPENED"]);
    let read = super::live::with_live_world(rig.app.world_mut(), |s| {
        s.eval::<String>(
            "local before = #(CHARZ_LOG or {}) CloseBankFrame() local t = {}              for i = before + 1, #(CHARZ_LOG or {}) do                if CHARZ_LOG[i]:find('BANKFRAME_CLOSED', 1, true) then t[#t + 1] = CHARZ_LOG[i]:match('bankopen=(%a+)') end end              return table.concat(t, ',') .. '|' .. tostring(GetContainerFreeSlots(-1) ~= nil)",
        )
    })
    .expect("a VM")
    .expect("the chunk runs");
    // The stock `BankFrame` hears the first fire ahead of the observer and its `OnHide` calls `CloseBankFrame` again, which fires twice with the banker set and clears it; the outer call's second fire is gone with the interact target.
    assert_eq!(read, "true,true,false|false");
    assert!(!rig
        .app
        .world()
        .non_send_resource::<crate::ui_bank::BankOpen>()
        .is_open());
}

/// `PurchaseSlot` (`0x57bbc0`): no banker, no control or seven slots send nothing and show nothing (`0x57bbeb`-`0x57bc10`); a coinage below the next row's price shows `ERR_BANKSLOT_INSUFFICIENT_FUNDS` inside the call (`0x57bc2a` -> `0x57bc2c`); a coinage at the price sends `CMSG_BUY_BANK_SLOT` naming the interact target, the banker (`0x57bc54`).
#[test]
fn purchase_slot_gates_refuses_and_sends_at_its_call() {
    use benilla_protocol::field::FIELD_PLAYER_FIELD_COINAGE;
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    let (tx, rx) = crossbeam_channel::unbounded();
    rig.app.insert_resource(crate::net::NetCommands(tx));
    let sends = |rx: &crossbeam_channel::Receiver<crate::net::ClientCommand>| {
        rx.try_iter()
            .filter_map(|c| match c {
                crate::net::ClientCommand::BuyBankSlot { guid } => Some(guid),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    // The refusal lines the call logged before it returned, then the marker.
    let call = |rig: &mut Rig| {
        super::live::with_live_world(rig.app.world_mut(), |s| {
            s.eval::<String>(
                "local before = #(CHARZ_LOG or {}) PurchaseSlot() local t = {} \
                 for i = before + 1, #(CHARZ_LOG or {}) do \
                   if CHARZ_LOG[i]:find('UI_ERROR_MESSAGE', 1, true) then \
                     t[#t + 1] = CHARZ_LOG[i]:match('\t([^\t]*)\t') end end \
                 t[#t + 1] = 'returned' return table.concat(t, '|')",
            )
        })
        .expect("a VM")
        .expect("the chunk runs")
    };
    let coinage = |c: u32| self_values(&[(FIELD_PLAYER_FIELD_COINAGE, c)]);
    rig.packets(vec![bank_slots(1), coinage(1_000_000_000)], 2);
    assert_eq!(call(&mut rig), "returned", "no banker");
    assert!(sends(&rx).is_empty());

    rig.packets(
        vec![
            banker_create(0x20000),
            SessionEvent::ShowBank { banker: BANKER },
        ],
        2,
    );
    let cost = rig_price(&rig, 1);
    assert!(cost > 0, "the price row after one slot");
    rig.packets(vec![coinage(cost - 1)], 2);
    assert_eq!(
        call(&mut rig),
        "You can't afford that.|returned",
        "one copper short: the line inside the call"
    );
    assert!(sends(&rx).is_empty());
    rig.packets(vec![coinage(cost)], 2);
    assert_eq!(call(&mut rig), "returned", "exactly the price");
    assert_eq!(sends(&rx), [BANKER]);

    // The stock `UIParent` closes every window on `PLAYER_CONTROL_LOST`, so the bank opens again with control lost.
    rig.packets(
        vec![control(false), SessionEvent::ShowBank { banker: BANKER }],
        2,
    );
    assert_eq!(call(&mut rig), "returned", "no control, at the price");
    assert!(sends(&rx).is_empty());
    rig.packets(vec![control(true), bank_slots(7), coinage(0)], 2);
    assert_eq!(call(&mut rig), "returned", "seven slots, even when short");
    assert!(sends(&rx).is_empty());
    rig.packets(vec![bank_slots(6), coinage(rig_price(&rig, 6))], 2);
    assert_eq!(call(&mut rig), "returned", "the sixth row's price");
    assert_eq!(sends(&rx), [BANKER]);
}

/// The next slot's price after `n` purchased, as the client's table holds it.
fn rig_price(rig: &Rig, n: u8) -> u32 {
    rig.app
        .world()
        .get_resource::<crate::ui_bank::BankPrices>()
        .and_then(|p| p.0.next_slot_price(n))
        .expect("BankBagSlotPrices.dbc")
}

/// Another window's open takes the interact target from the banker: the setter's interaction close fires `BANKFRAME_CLOSED` and clears the banker ahead of that window (`0x51fce0` -> `0x51fd1c` -> `0x512fa7`); a vendor's empty refusal opens nothing and leaves the bank open (`0x6d9ad2`-`0x6d9af8`).
#[test]
fn another_window_taking_the_interact_target_closes_the_bank() {
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    rig.packets(
        vec![
            banker_create(0x20000),
            SessionEvent::ShowBank { banker: BANKER },
        ],
        2,
    );
    let refusal = SessionEvent::VendorInventory {
        vendor: BANKER + 1,
        items: Vec::new(),
        error: Some(2),
    };
    let seen = rig.packets(vec![refusal], 2);
    assert!(fired(&seen, BANK_EVENTS).is_empty(), "nothing opened");
    let seen = rig.packets(
        vec![SessionEvent::VendorInventory {
            vendor: BANKER + 1,
            items: Vec::new(),
            error: Some(0),
        }],
        2,
    );
    let order: Vec<String> = seen
        .iter()
        .filter(|s| ["BANKFRAME_CLOSED", "MERCHANT_SHOW"].contains(&s.event.as_str()))
        .map(|s| s.event.clone())
        .collect();
    assert_eq!(
        order.first().map(String::as_str),
        Some("BANKFRAME_CLOSED"),
        "the bank closes ahead of the vendor: {order:?}"
    );
    assert!(!rig
        .app
        .world()
        .non_send_resource::<crate::ui_bank::BankOpen>()
        .is_open());
    let seen = rig.packets(vec![SessionEvent::ShowBank { banker: BANKER }], 2);
    assert_eq!(
        fired(&seen, &["BANKFRAME_OPENED", "BANKFRAME_CLOSED"]),
        ["BANKFRAME_OPENED"],
        "the next open finds no bank to close"
    );
    // A dialog that fires in its own handler: the setter runs at the start of its open, so the bank closes first (`0x58c5a0`).
    let seen = rig.packets(
        vec![
            npc_create(BANKER + 2, 0),
            SessionEvent::QuestDetail(benilla_protocol::messages::QuestDetails {
                npc: BANKER + 2,
                quest_id: QUEST,
                title: "Charz".into(),
                ..Default::default()
            }),
        ],
        2,
    );
    assert_eq!(
        fired(&seen, &["BANKFRAME_CLOSED", "QUEST_DETAIL"])
            .first()
            .map(String::as_str),
        Some("BANKFRAME_CLOSED")
    );
    assert_eq!(fired(&seen, &["QUEST_DETAIL"]).len(), 1);
}

/// A window opened on the banker itself keeps the interact target: the setter zeroes it before the bank's leg runs (`0x512e89`), so the bank's close finds no target and closes nothing else (`0x57b8f1`); the gossip menu then closes at the next bank open (`0x51fce0` -> `0x512ef6`).
#[test]
fn a_window_on_the_banker_itself_stays_open_after_the_bank_closes() {
    use crate::ui_session::NpcSession;
    use benilla_protocol::messages::GossipMenu;
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    let gossip_npc = |rig: &Rig| {
        rig.app
            .world()
            .resource::<crate::ui_gossip::GossipState>()
            .npc
    };
    rig.packets(
        vec![
            banker_create(0x20001),
            SessionEvent::ShowBank { banker: BANKER },
        ],
        2,
    );
    let menu = SessionEvent::GossipMenu(GossipMenu {
        npc: BANKER,
        menu_id: 1,
        text_id: 1,
        options: Vec::new(),
        quests: Vec::new(),
    });
    let (tx, _rx) = crossbeam_channel::unbounded();
    rig.app.insert_resource(crate::net::NetCommands(tx));
    let greeting = SessionEvent::NpcGreeting {
        text_id: 1,
        blocks: vec![benilla_protocol::messages::NpcTextBlock {
            probability: 1.0,
            male: "Charz".into(),
            ..Default::default()
        }],
    };
    let seen = rig.packets(vec![menu, greeting], 4);
    assert_eq!(
        fired(&seen, &["BANKFRAME_CLOSED", "GOSSIP_SHOW", "GOSSIP_CLOSED"]),
        ["BANKFRAME_CLOSED", "BANKFRAME_CLOSED", "GOSSIP_SHOW"],
        "the stock frame's OnHide closes the bank again inside the first fire"
    );
    assert_eq!(gossip_npc(&rig), Some(BANKER), "the menu keeps its state");
    let seen = rig.packets(vec![SessionEvent::ShowBank { banker: BANKER }], 2);
    assert_eq!(
        fired(&seen, &["GOSSIP_CLOSED", "BANKFRAME_OPENED"]),
        ["GOSSIP_CLOSED", "BANKFRAME_OPENED"],
        "the bank's setter closes the menu it finds before the bank opens (`0x57b909` -> `0x512ef6`)"
    );
    assert_eq!(gossip_npc(&rig), None);
    // A vendor on the banker: the same.
    let seen = rig.packets(
        vec![SessionEvent::VendorInventory {
            vendor: BANKER,
            items: Vec::new(),
            error: Some(0),
        }],
        4,
    );
    assert_eq!(
        fired(
            &seen,
            &["BANKFRAME_CLOSED", "MERCHANT_SHOW", "MERCHANT_CLOSED"]
        ),
        ["BANKFRAME_CLOSED", "BANKFRAME_CLOSED", "MERCHANT_SHOW"]
    );
    assert_eq!(
        rig.app
            .world()
            .resource::<crate::ui_merchant::MerchantOpen>()
            .npc(),
        Some(BANKER)
    );
}

/// Only an open that reaches the setter closes the bank: a taxi map with one known node shows `ERR_TAXINOPATHS` instead (`0x6d61b5`-`0x6d61d1`); a quest panel from a giver not held opens without it (`0x58c5c7`); a guild bank list closes it only as the answer to a hello with no banker open (`0x5a76b3`-`0x5a772b`); the random battleground's list from a battlemaster takes it even for the PvP window (`0x54e509`-`0x54e52e`); a mailbox opened in the client takes it before `MAIL_SHOW` (`0x56da67`).
#[test]
fn only_an_open_reaching_the_setter_closes_the_bank() {
    use benilla_protocol::messages::{BattlefieldList, GuildBankList, TaxiMask};
    const CLOSED: &[&str] = &["BANKFRAME_CLOSED"];
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    let (tx, _rx) = crossbeam_channel::unbounded();
    rig.app.insert_resource(crate::net::NetCommands(tx));
    let open = |rig: &mut Rig| {
        rig.packets(vec![SessionEvent::ShowBank { banker: BANKER }], 2);
        assert!(rig
            .app
            .world()
            .non_send_resource::<crate::ui_bank::BankOpen>()
            .is_open());
    };
    let closes = |seen: &[Seen], before: &str| -> bool {
        let order = fired(seen, &["BANKFRAME_CLOSED", before]);
        order.first().map(String::as_str) == Some("BANKFRAME_CLOSED")
    };
    rig.packets(vec![banker_create(0x20000)], 2);
    open(&mut rig);

    let taxi = |nodes: &[u32]| {
        let mut known_mask = TaxiMask::default();
        for &n in nodes {
            known_mask.set(n);
        }
        SessionEvent::TaxiNodesShown {
            flightmaster: BANKER + 3,
            nearest_node: 2,
            known_mask,
        }
    };
    let seen = rig.packets(vec![taxi(&[2])], 2);
    assert!(
        fired(&seen, CLOSED).is_empty(),
        "one known node: no map, no setter"
    );
    assert_eq!(
        fired(&seen, &["UI_ERROR_MESSAGE"]).len(),
        1,
        "ERR_TAXINOPATHS"
    );
    let seen = rig.packets(vec![taxi(&[2, 4])], 2);
    assert!(
        closes(&seen, "TAXIMAP_OPENED"),
        "{:?}",
        fired(&seen, &["BANKFRAME_CLOSED", "TAXIMAP_OPENED"])
    );
    open(&mut rig);

    let greeting = |npc: u64| {
        SessionEvent::QuestGreeting(benilla_protocol::messages::QuestGiverList {
            npc,
            ..Default::default()
        })
    };
    let seen = rig.packets(vec![greeting(BANKER + 4)], 2);
    assert!(fired(&seen, CLOSED).is_empty(), "a giver not held");
    assert_eq!(fired(&seen, &["QUEST_GREETING"]).len(), 1);
    let seen = rig.packets(vec![npc_create(BANKER + 4, 0), greeting(BANKER + 4)], 2);
    assert!(closes(&seen, "QUEST_GREETING"));
    open(&mut rig);

    let list = || SessionEvent::GuildBankList(GuildBankList::default());
    let seen = rig.packets(vec![list()], 2);
    assert!(fired(&seen, CLOSED).is_empty(), "no hello: nothing opens");
    {
        let world = rig.app.world_mut();
        let net = crate::net::NetCommands(world.resource::<crate::net::NetCommands>().0.clone());
        world
            .non_send_resource_mut::<crate::ui_guild_bank::GuildBankState>()
            .talk(BANKER + 5, &net);
    }
    let seen = rig.packets(vec![list()], 2);
    assert!(
        closes(&seen, "GUILDBANKFRAME_OPENED"),
        "{:?}",
        fired(&seen, &["BANKFRAME_CLOSED", "GUILDBANKFRAME_OPENED"])
    );
    open(&mut rig);
    let seen = rig.packets(vec![list()], 2);
    assert!(
        fired(&seen, CLOSED).is_empty(),
        "a list for the open guild bank opens nothing"
    );

    let bf = |from_anywhere: bool, bg_type: u32| {
        SessionEvent::BattlefieldList(BattlefieldList {
            battlemaster: BANKER + 6,
            from_anywhere,
            bg_type,
            instances: vec![1],
            ..Default::default()
        })
    };
    let seen = rig.packets(vec![bf(true, 2)], 2);
    assert!(fired(&seen, CLOSED).is_empty(), "a list for the PvP window");
    let seen = rig.packets(vec![bf(true, benilla_ui::script::RANDOM_BATTLEGROUND)], 2);
    assert!(closes(&seen, "NPC_PVPQUEUE_ANYWHERE"));
    open(&mut rig);

    rig.app
        .world_mut()
        .resource_mut::<crate::ui_mail::MailOpen>()
        .click(BANKER + 7);
    let seen = rig.packets(Vec::new(), 2);
    assert!(
        closes(&seen, "MAIL_SHOW"),
        "{:?}",
        fired(&seen, &["BANKFRAME_CLOSED", "MAIL_SHOW"])
    );
    assert!(!rig
        .app
        .world()
        .non_send_resource::<crate::ui_bank::BankOpen>()
        .is_open());
}

/// The interact target's clears outside the windows close the open bank through its leg (`0x512fa7`): our own death (`0x6dc17a`), ahead of `PLAYER_DEAD`; the banker's death (`0x7293bf`); and a form without the NPC-interact bit (`0x6da1dc`), where a stance or a form with the bit keeps it.
#[test]
fn the_interact_target_clears_close_the_bank() {
    use benilla_protocol::field::FIELD_UNIT_BYTES_2;
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    let open = |rig: &mut Rig| {
        rig.packets(vec![SessionEvent::ShowBank { banker: BANKER }], 2);
    };
    let closed = |seen: &[Seen]| pin(seen, &["BANKFRAME_CLOSED", "PLAYER_DEAD"], &["bankopen"]);
    rig.packets(vec![banker_create(0x20000)], 1);
    open(&mut rig);
    let seen = rig.packets(vec![self_values(&[(FIELD_UNIT_HEALTH, 0)])], 3);
    assert_eq!(
        closed(&seen),
        [
            "BANKFRAME_CLOSED | bankopen=true",
            "BANKFRAME_CLOSED | bankopen=false",
            "PLAYER_DEAD | bankopen=false",
        ],
        "our death: the stock OnHide closes it again inside the first fire"
    );
    rig.packets(vec![self_values(&[(FIELD_UNIT_HEALTH, 100)])], 2);
    open(&mut rig);
    let seen = rig.packets(
        vec![SessionEvent::ObjectValues {
            guid: BANKER,
            fields: ObjectFields::from_pairs(&[(FIELD_UNIT_HEALTH, 0)]),
        }],
        2,
    );
    assert_eq!(
        closed(&seen),
        [
            "BANKFRAME_CLOSED | bankopen=true",
            "BANKFRAME_CLOSED | bankopen=false",
        ],
        "the banker's death"
    );
    let forms: Vec<(u32, u32)> = rig
        .app
        .world()
        .get_resource::<crate::ui_action::Spells>()
        .map(|s| s.forms.iter().map(|(&id, f)| (id, f.flags)).collect())
        .unwrap_or_default();
    let keeps = forms.iter().find(|(id, f)| *id < 256 && f & 0x9 != 0);
    let clears = forms.iter().find(|(id, f)| *id < 256 && f & 0x9 == 0);
    let (Some(&(keeps, _)), Some(&(clears, _))) = (keeps, clears) else {
        panic!("SpellShapeshiftForm has a form of each kind: {forms:?}");
    };
    rig.packets(
        vec![SessionEvent::ObjectValues {
            guid: BANKER,
            fields: ObjectFields::from_pairs(&[(FIELD_UNIT_HEALTH, 500)]),
        }],
        1,
    );
    open(&mut rig);
    let seen = rig.packets(vec![self_values(&[(FIELD_UNIT_BYTES_2, keeps << 24)])], 2);
    assert!(closed(&seen).is_empty(), "form {keeps} keeps the bank");
    let seen = rig.packets(vec![self_values(&[(FIELD_UNIT_BYTES_2, clears << 24)])], 2);
    assert_eq!(
        closed(&seen),
        [
            "BANKFRAME_CLOSED | bankopen=true",
            "BANKFRAME_CLOSED | bankopen=false",
        ],
        "form {clears} clears the interact target"
    );
}

/// `SMSG_DESTROY_OBJECT` with its death byte on the living banker closes the bank inside the destroy's handler (`0x4d7682` -> `0x7293bf`), ahead of the next packet's events; on a banker already at 0 health it runs no death.
#[test]
fn the_bankers_destroy_on_death_closes_the_bank_in_its_handler() {
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    let events = &["BANKFRAME_CLOSED", "UI_ERROR_MESSAGE"];
    rig.packets(
        vec![
            banker_create(0x20000),
            SessionEvent::ShowBank { banker: BANKER },
        ],
        2,
    );
    let seen = rig.packets(
        vec![
            SessionEvent::DestroyedOnDeath(BANKER),
            SessionEvent::ObjectDestroyed(BANKER),
            SessionEvent::BuyBankSlotResult { result: 1 },
        ],
        2,
    );
    assert_eq!(
        pin(&seen, events, &["bankopen"]),
        [
            "BANKFRAME_CLOSED | bankopen=true",
            "BANKFRAME_CLOSED | bankopen=false",
            "UI_ERROR_MESSAGE You can't afford that. | bankopen=false",
        ]
    );
    // A banker destroyed at 0 health with no interaction held runs nothing more.
    rig.packets(
        vec![
            banker_create(0x20000),
            SessionEvent::ShowBank { banker: BANKER },
            SessionEvent::ObjectValues {
                guid: BANKER,
                fields: ObjectFields::from_pairs(&[(FIELD_UNIT_HEALTH, 0)]),
            },
        ],
        2,
    );
    let seen = rig.packets(
        vec![
            SessionEvent::DestroyedOnDeath(BANKER),
            SessionEvent::ObjectDestroyed(BANKER),
        ],
        2,
    );
    assert!(pin(&seen, events, &[]).is_empty(), "{seen:?}");
}

/// The banker flag lost on the open bank's banker closes it from the NPC-flag watch (`0x71a1a8`-`0x71a1db`): `BANKFRAME_CLOSED` with the banker set, again from the interaction close, then the banker clears.
#[test]
fn the_banker_flag_lost_closes_the_bank_from_the_flag_watch() {
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    rig.packets(
        vec![
            banker_create(0x20000),
            SessionEvent::ShowBank { banker: BANKER },
        ],
        2,
    );
    let seen = rig.packets(
        vec![SessionEvent::ObjectValues {
            guid: BANKER,
            fields: ObjectFields::from_pairs(&[(benilla_protocol::field::FIELD_UNIT_NPC_FLAGS, 0)]),
        }],
        2,
    );
    assert_eq!(
        pin(&seen, BANK_EVENTS, &["bankopen"]),
        [
            "BANKFRAME_CLOSED | bankopen=true",
            "BANKFRAME_CLOSED | bankopen=true",
            "BANKFRAME_CLOSED | bankopen=false",
        ],
        "the stock frame's OnHide closes it inside the first fire, as `CloseBankFrame` does"
    );
    assert!(!rig
        .app
        .world()
        .non_send_resource::<crate::ui_bank::BankOpen>()
        .is_open());
}

/// Brown Horse (458) rides creature 284, Bombay Cat (10673) summons creature 7385.
const HORSE: u32 = 458;
const CAT: u32 = 10673;

fn creature_answer(entry: u32, name: &str) -> SessionEvent {
    SessionEvent::CreatureName {
        entry,
        name: Some(name.into()),
        subname: None,
        cursor_name: None,
        creature_type: None,
        pet_family: 0,
        rank: 0,
        type_flags: 0,
        display_id: 0,
        civilian: false,
        racial_leader: false,
        modifiers: (0.0, 0.0),
        movement_id: 0,
    }
}

fn own_aura(slot: u8, spell_id: u32) -> SessionEvent {
    own_auras(false, &[(slot, spell_id)])
}

/// An aura packet for us, `SMSG_AURA_UPDATE_ALL` with `all`.
fn own_auras(all: bool, slots: &[(u8, u32)]) -> SessionEvent {
    SessionEvent::AuraUpdate(benilla_protocol::messages::AuraUpdate {
        unit: ME,
        all,
        slots: slots
            .iter()
            .map(|&(slot, spell_id)| {
                (
                    slot,
                    (spell_id != 0).then_some(benilla_protocol::messages::AuraRecord {
                        spell_id,
                        flags: 0x1,
                        level: 80,
                        stacks: 1,
                        caster: None,
                        duration_ms: None,
                    }),
                )
            })
            .collect(),
    })
}

/// `SMSG_INITIAL_SPELLS` with `spell_ids`.
fn book(spell_ids: &[u32]) -> SessionEvent {
    SessionEvent::SpellBook {
        spell_ids: spell_ids.to_vec(),
        cooldowns: Vec::new(),
    }
}

/// The companion lists fill as the spell handlers add the known spells (`0x5426a5`) and every companion event fires from the handler that causes it: our create's init replays the book and its update waits on the mount's record, firing from its answer (`0x6e7f50`, `0x53ebc0`); a live learn's line and `COMPANION_LEARNED` wait on the critter's; the mount aura's arrival and removal fire `"MOUNT"` (`0x724a6b`, `0x71e9b2`), and an `ALL` packet re-applies the mount we hold (`0x72f904`); an unlearn updates and fires `COMPANION_UNLEARNED` (`0x53fad0`).
#[test]
fn companion_events_fire_from_the_spell_and_creature_handlers() {
    // `OBJECT_FIELD_TYPE` with the unit bit: the aura handler skips a non-unit (`0x4d4db0`).
    let Some(mut rig) = Rig::login_after(vec![book(&[HORSE])], &[(2, 0x19)]) else {
        return;
    };
    let events = &[
        "COMPANION_UPDATE",
        "COMPANION_LEARNED",
        "COMPANION_UNLEARNED",
    ];
    let verbs = &["mounts", "critters", "mount1"];
    let seen = rig.packets(
        vec![
            creature_answer(284, "Brown Horse"),
            SessionEvent::SpellLearned { spell_id: CAT },
            creature_answer(7385, "Bombay Cat"),
            own_aura(3, HORSE),
            own_auras(true, &[(3, HORSE)]),
            own_aura(3, 0),
            SessionEvent::SpellRemoved { spell_id: CAT },
        ],
        3,
    );
    assert_eq!(
        pin(&seen, events, verbs),
        vec![
            "COMPANION_UPDATE | mounts=1 critters=0 mount1=Brown Horse",
            "COMPANION_UPDATE | mounts=1 critters=1 mount1=Brown Horse",
            "COMPANION_LEARNED | mounts=1 critters=1 mount1=Brown Horse",
            "COMPANION_UPDATE MOUNT | mounts=1 critters=1 mount1=Brown Horse",
            "COMPANION_UPDATE MOUNT | mounts=1 critters=1 mount1=Brown Horse",
            "COMPANION_UPDATE MOUNT | mounts=1 critters=1 mount1=Brown Horse",
            "COMPANION_UPDATE | mounts=1 critters=0 mount1=Brown Horse",
            "COMPANION_UNLEARNED | mounts=1 critters=0 mount1=Brown Horse",
        ]
    );
}

/// Our object's re-create, as a far teleport makes it, runs the init again: the lists clear and refill from the buffer the login set, and with the record cached the update fires at once (`0x6e8378` -> `0x6e7fe3`). A spell learned live since never entered the buffer (`0x6e7d60`).
#[test]
fn our_recreate_replays_the_login_book_into_the_companion_lists() {
    let Some(mut rig) = Rig::login_after(vec![book(&[HORSE])], &[(2, 0x19)]) else {
        return;
    };
    let events = &["COMPANION_UPDATE", "COMPANION_LEARNED"];
    let verbs = &["mounts", "critters", "mount1"];
    let seen = rig.packets(
        vec![
            creature_answer(284, "Brown Horse"),
            creature_answer(7385, "Bombay Cat"),
            SessionEvent::SpellLearned { spell_id: CAT },
        ],
        3,
    );
    assert_eq!(
        pin(&seen, events, verbs),
        vec![
            "COMPANION_UPDATE | mounts=1 critters=0 mount1=Brown Horse",
            "COMPANION_UPDATE | mounts=1 critters=1 mount1=Brown Horse",
            "COMPANION_LEARNED | mounts=1 critters=1 mount1=Brown Horse",
        ]
    );
    let seen = rig.packets(
        vec![SessionEvent::ObjectDestroyed(ME), self_create(&[(2, 0x19)])],
        3,
    );
    assert_eq!(
        pin(&seen, events, verbs),
        vec!["COMPANION_UPDATE | mounts=1 critters=0 mount1=Brown Horse"]
    );
}

/// A learn or an unlearn landing in the same frame as our create, after it, files into the lists at once: the handlers ask the object manager for our object, which holds it from the create on (`0x6e7d60`, `0x6e7840`), not from the frame's player tag.
#[test]
fn a_learn_and_an_unlearn_after_our_create_in_one_frame_reach_the_lists() {
    let Some(mut rig) = Rig::login_after(vec![book(&[HORSE])], &[(2, 0x19)]) else {
        return;
    };
    rig.packets(
        vec![
            creature_answer(284, "Brown Horse"),
            creature_answer(7385, "Bombay Cat"),
        ],
        3,
    );
    let events = &[
        "COMPANION_UPDATE",
        "COMPANION_LEARNED",
        "COMPANION_UNLEARNED",
    ];
    let verbs = &["mounts", "critters"];
    let seen = rig.packets(
        vec![
            SessionEvent::ObjectDestroyed(ME),
            self_create(&[(2, 0x19)]),
            SessionEvent::SpellLearned { spell_id: CAT },
        ],
        3,
    );
    assert_eq!(
        pin(&seen, events, verbs),
        vec![
            "COMPANION_UPDATE | mounts=1 critters=0",
            "COMPANION_UPDATE | mounts=1 critters=1",
            "COMPANION_LEARNED | mounts=1 critters=1",
        ]
    );
    let seen = rig.packets(
        vec![
            SessionEvent::ObjectDestroyed(ME),
            self_create(&[(2, 0x19)]),
            SessionEvent::SpellRemoved { spell_id: HORSE },
        ],
        3,
    );
    assert_eq!(
        pin(&seen, events, verbs),
        vec![
            "COMPANION_UPDATE | mounts=1 critters=0",
            // Both lists empty: the update sorts nothing and stays silent.
            "COMPANION_UNLEARNED | mounts=0 critters=0",
        ]
    );
}

/// The bar's `PET_BAR_UPDATE` handler re-tests the Pets tab off `GetNumCompanions` (#1244): it reads the lists the spell handler filled, whichever of the mount's record and the pet packet lands first. `COMPANION_UPDATE` fires from the record's answer, `PET_BAR_UPDATE` from the pet packet's setter (`0x5d6ad2` -> `0x5d4c1b`), each in its own packet's turn.
#[test]
fn the_pet_bar_reads_the_mounts_the_spell_handler_filed_in_both_orders() {
    let events = &["COMPANION_UPDATE", "PET_BAR_UPDATE"];
    let verbs = &["mounts", "petui"];
    let bar = || {
        SessionEvent::PetSpells(Box::new(benilla_protocol::messages::PetSpells {
            pet_guid: PET,
            ..Default::default()
        }))
    };
    let Some(mut rig) = Rig::login_after(vec![book(&[HORSE])], &[]) else {
        return;
    };
    let answer_last = rig.packets(vec![bar(), creature_answer(284, "Brown Horse")], 3);
    assert_eq!(
        pin(&answer_last, events, verbs),
        vec![
            "PET_BAR_UPDATE | mounts=1 petui=nil",
            "COMPANION_UPDATE | mounts=1 petui=nil",
        ]
    );
    drop(rig);
    let Some(mut rig) = Rig::login_after(vec![book(&[HORSE])], &[]) else {
        return;
    };
    let answer_first = rig.packets(vec![creature_answer(284, "Brown Horse"), bar()], 3);
    assert_eq!(
        pin(&answer_first, events, verbs),
        vec![
            "COMPANION_UPDATE | mounts=1 petui=nil",
            "PET_BAR_UPDATE | mounts=1 petui=nil",
        ]
    );
}

/// A Hunter's pet: its unit type, pet number, creator and summoner us, happiness, experience and the abandon flag.
fn hunter_pet(happiness: u32, xp: u32) -> SessionEvent {
    use benilla_protocol::field::{
        FIELD_UNIT_BYTES_2, FIELD_UNIT_CREATEDBY, FIELD_UNIT_PETEXPERIENCE,
        FIELD_UNIT_PETNEXTLEVELEXP, FIELD_UNIT_PETNUMBER, FIELD_UNIT_POWER1, FIELD_UNIT_SUMMONEDBY,
    };
    SessionEvent::ObjectCreate {
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
            (2, 0x9),
            (FIELD_UNIT_HEALTH, 500),
            (FIELD_UNIT_MAXHEALTH, 500),
            (FIELD_UNIT_LEVEL, 80),
            (FIELD_UNIT_PETNUMBER, 42),
            (FIELD_UNIT_CREATEDBY, ME as u32),
            (FIELD_UNIT_CREATEDBY + 1, (ME >> 32) as u32),
            (FIELD_UNIT_SUMMONEDBY, ME as u32),
            (FIELD_UNIT_SUMMONEDBY + 1, (ME >> 32) as u32),
            (FIELD_UNIT_POWER1 + 4, happiness),
            (FIELD_UNIT_PETEXPERIENCE, xp),
            (FIELD_UNIT_PETNEXTLEVELEXP, 8000),
            // The pet flags' abandon bit (`UNIT_FIELD_BYTES_2` byte 2).
            (
                FIELD_UNIT_BYTES_2,
                u32::from(crate::ui_pet::PET_FLAG_ABANDON) << 16,
            ),
        ])
        .into_created(ObjectType::Unit),
    }
}

/// The pet frame's unit signals fire from the field handler of the unit the `"pet"` token names, one per changed field, each after the delta applies (`0x614190`, event slots 23 and 71-72): `UNIT_HAPPINESS` on every happiness change, the bucket kept or not, and `UNIT_PET_EXPERIENCE` on the experience fields; the stat and menu verbs read the pet list's head at each.
#[test]
fn pet_stat_events_fire_from_the_pets_field_handler() {
    use benilla_protocol::field::{
        FIELD_UNIT_BYTES_0, FIELD_UNIT_PETEXPERIENCE, FIELD_UNIT_POWER1,
    };
    // A Hunter (class 3), held as a player so the creator test finds it (`0x71b656`).
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER, (FIELD_UNIT_BYTES_0, 1 | 3 << 8)]) else {
        return;
    };
    let events = &["UNIT_HAPPINESS", "UNIT_PET_EXPERIENCE"];
    let verbs = &["petui", "happy", "xp", "abandon"];
    rig.packets(
        vec![
            hunter_pet(1_000_000, 4200),
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
    let pet_values = |pairs: &[(u16, u32)]| SessionEvent::ObjectValues {
        guid: PET,
        fields: ObjectFields::from_pairs(pairs),
    };
    let seen = rig.packets(
        vec![
            pet_values(&[(FIELD_UNIT_POWER1 + 4, 999_999)]),
            pet_values(&[(FIELD_UNIT_POWER1 + 4, 100_000)]),
            pet_values(&[(FIELD_UNIT_PETEXPERIENCE, 5000)]),
        ],
        3,
    );
    assert_eq!(
        pin(&seen, events, verbs),
        vec![
            "UNIT_HAPPINESS pet | petui=1 happy=3 xp=4200/8000 abandon=1",
            "UNIT_HAPPINESS pet | petui=1 happy=1 xp=4200/8000 abandon=1",
            "UNIT_PET_EXPERIENCE pet | petui=1 happy=1 xp=5000/8000 abandon=1",
        ]
    );
}

/// `PET_ATTACK_START`/`STOP` fire from the flags field handler of a unit we charm, else summon, on each move of its `0x800` bit, in field order with the pet's other signals (`0x73f29b` -> `0x73c503`-`0x73c53d` -> `0x5d30a0`); another bit, or another owner's unit, fires none.
#[test]
fn pet_attack_events_fire_from_the_flags_field_handler() {
    use benilla_protocol::field::{FIELD_UNIT_POWER1, FIELD_UNIT_SUMMONEDBY};
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    rig.packets(
        vec![
            hunter_pet(1_000_000, 4200),
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
    let pet_values = |pairs: &[(u16, u32)]| SessionEvent::ObjectValues {
        guid: PET,
        fields: ObjectFields::from_pairs(pairs),
    };
    let events = &["PET_ATTACK_START", "PET_ATTACK_STOP", "UNIT_HAPPINESS"];
    let seen = rig.packets(
        vec![
            pet_values(&[(FIELD_UNIT_FLAGS, 0x800), (FIELD_UNIT_POWER1 + 4, 999_999)]),
            pet_values(&[(FIELD_UNIT_FLAGS, 0x1800)]),
            pet_values(&[(FIELD_UNIT_FLAGS, 0x1000)]),
            pet_values(&[
                (FIELD_UNIT_SUMMONEDBY, 0x99),
                (FIELD_UNIT_SUMMONEDBY + 1, 0),
                (FIELD_UNIT_FLAGS, 0x800),
            ]),
        ],
        3,
    );
    // The setter writes the pet-attacking global after its signal (`0x5d30b2` then `0x5d30ba`), so a handler reads the old value; `IsPetAttackActive` reads the global whatever its argument.
    assert_eq!(
        pin(&seen, events, &["petattack"]),
        vec![
            "UNIT_HAPPINESS pet | petattack=false",
            "PET_ATTACK_START | petattack=false",
            "PET_ATTACK_STOP | petattack=true",
        ]
    );
}

/// The pet number's field handler (`0x72cff0`) runs on each move of a held unit's `UNIT_FIELD_PETNUMBER` after the delta applies: `UNIT_CLASSIFICATION_CHANGED` to each token naming the unit (`0x72d034`), then `PET_UI_UPDATE` only for the pet list's head (`0x72d041`, `0x72d05f`); another unit's move fires no `PET_UI_UPDATE`, an unheld guid's delta reaches no handler, and the name re-read asks the new number's name (`0x72d01c`).
#[test]
fn the_pet_number_handler_signals_its_tokens_and_updates_the_pet_ui() {
    use benilla_protocol::field::FIELD_UNIT_PETNUMBER;
    const OTHER: u64 = 0xF130_0000_0000_0099;
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    let other = SessionEvent::ObjectCreate {
        guid: OTHER,
        kind: EntityKind::Unit,
        display_id: None,
        position: [-8950.0, -133.0, 83.5],
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
            (FIELD_UNIT_HEALTH, 100),
            (FIELD_UNIT_MAXHEALTH, 100),
            (FIELD_UNIT_LEVEL, 10),
        ])
        .into_created(ObjectType::Unit),
    };
    let pet_name = |pet_number: u32, name: &str| SessionEvent::PetName {
        pet_number,
        name: name.into(),
        declensions: None,
    };
    rig.packets(
        vec![
            pet_name(42, "Rex"),
            pet_name(43, "Fang"),
            hunter_pet(1_000_000, 4200),
            other,
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
    assert_eq!(rig.now().line(&["petname"]), "NOW | petname=Rex");
    let values = |guid: u64, number: u32| SessionEvent::ObjectValues {
        guid,
        fields: ObjectFields::from_pairs(&[(FIELD_UNIT_PETNUMBER, number)]),
    };
    let events = &["UNIT_CLASSIFICATION_CHANGED", "PET_UI_UPDATE"];
    let seen = rig.packets(
        vec![
            values(PET, 43),
            values(OTHER, 7),
            values(0xF130_0000_0000_0123, 5),
        ],
        3,
    );
    // `UnitName` reads the units' pushed snapshot until #1289; 12340 re-reads the name first (`0x72d01c`), so it answers Fang at both.
    assert_eq!(
        pin(&seen, events, &["petui", "petname"]),
        vec![
            "UNIT_CLASSIFICATION_CHANGED pet | petui=1 petname=Rex",
            "PET_UI_UPDATE | petui=1 petname=Rex",
        ]
    );
    assert_eq!(rig.now().line(&["petname"]), "NOW | petname=Fang");
}

/// An empty `SMSG_PET_GUIDS` (`0x5d6550`) empties the list and leaves the book: `HasPetSpells` reads the count the last `SMSG_PET_SPELLS` left (`0x53dd34`), until a zero-guid one clears it (`0x5d6d88`).
#[test]
fn the_pet_book_outlives_an_empty_pet_list() {
    use benilla_protocol::messages::{PetActionEntry, PetSpells};
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    let mut spells = PetSpells {
        pet_guid: PET,
        ..Default::default()
    };
    // Growl, a book spell, as its packed word.
    spells.spells = vec![PetActionEntry::from(0xC100_0000 | 2649)];
    rig.packets(
        vec![
            hunter_pet(1_000_000, 4200),
            self_values(&[
                (FIELD_UNIT_SUMMON, PET as u32),
                (FIELD_UNIT_SUMMON + 1, (PET >> 32) as u32),
            ]),
            SessionEvent::PetSpells(Box::new(spells)),
        ],
        3,
    );
    assert_eq!(
        rig.live_now().line(&["petbar", "petbook"]),
        "NOW | petbar=1 petbook=1"
    );
    rig.packets(vec![SessionEvent::PetGuids(Vec::new())], 3);
    assert_eq!(
        rig.live_now().line(&["petbar", "petbook"]),
        "NOW | petbar=nil petbook=1"
    );
    rig.packets(vec![SessionEvent::PetSpells(Box::default())], 3);
    assert_eq!(
        rig.live_now().line(&["petbar", "petbook"]),
        "NOW | petbar=nil petbook=nil"
    );
}

/// The pet's packets fire their events from their handlers in wire order, each after its change: `SMSG_PET_MODE`'s `PET_BAR_UPDATE` (`0x5d3689`), a pet-addressed `SMSG_SPELL_COOLDOWN`'s flush (`0x80702e`, `0x807039`), and `SMSG_PET_SPELLS`' temporary bar `PET_BAR_UPDATE` then `SPELLS_CHANGED` (`0x5d6ad2`, `0x5d6d70`); both orders of the first two.
#[test]
fn the_pet_packets_fire_their_events_in_wire_order() {
    use benilla_protocol::messages::{PetActionEntry, PetMode, PetSpells};
    const GROWL: u32 = 2649;
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    let bar = |state: u32| {
        let mut spells = PetSpells {
            pet_guid: PET,
            state,
            ..Default::default()
        };
        for (i, word) in [
            0x0700_0002,
            0x0700_0001,
            0x0700_0000,
            0xC100_0000 | GROWL,
            0,
            0,
            0,
            0x0600_0002,
            0x0600_0001,
            0x0600_0000,
        ]
        .into_iter()
        .enumerate()
        {
            spells.bar[i] = PetActionEntry::from(word);
        }
        SessionEvent::PetSpells(Box::new(spells))
    };
    rig.packets(
        vec![
            hunter_pet(1_000_000, 4200),
            self_values(&[
                (FIELD_UNIT_SUMMON, PET as u32),
                (FIELD_UNIT_SUMMON + 1, (PET >> 32) as u32),
            ]),
            bar(0x0001),
        ],
        3,
    );
    let mode = SessionEvent::PetMode(PetMode {
        pet_guid: PET,
        state: 0x0101,
    });
    let cooldown = SessionEvent::SpellCooldowns {
        caster: PET,
        flags: 0,
        cooldowns: vec![(GROWL, 30_000)],
    };
    let events = &[
        "PET_BAR_UPDATE",
        "ACTIONBAR_UPDATE_COOLDOWN",
        "SPELL_UPDATE_COOLDOWN",
        "PET_BAR_UPDATE_COOLDOWN",
        "SPELLS_CHANGED",
        "UNIT_PET",
    ];
    let verbs = &["petfollow", "petcd"];
    let seen = rig.packets(vec![mode.clone(), cooldown.clone(), bar(0x0001)], 3);
    assert_eq!(
        pin(&seen, events, verbs),
        vec![
            "PET_BAR_UPDATE | petfollow=1 petcd=false",
            "ACTIONBAR_UPDATE_COOLDOWN | petfollow=1 petcd=true",
            "SPELL_UPDATE_COOLDOWN | petfollow=1 petcd=true",
            "PET_BAR_UPDATE_COOLDOWN | petfollow=1 petcd=true",
            "PET_BAR_UPDATE | petfollow=nil petcd=false",
            "SPELLS_CHANGED | petfollow=nil petcd=false",
        ]
    );
    let seen = rig.packets(vec![cooldown, mode], 3);
    assert_eq!(
        pin(&seen, events, verbs),
        vec![
            "ACTIONBAR_UPDATE_COOLDOWN | petfollow=nil petcd=true",
            "SPELL_UPDATE_COOLDOWN | petfollow=nil petcd=true",
            "PET_BAR_UPDATE_COOLDOWN | petfollow=nil petcd=true",
            "PET_BAR_UPDATE | petfollow=1 petcd=true",
        ]
    );
}

/// A refused pet order or cast refunds the start cooldown on the pet history (`0x804780`, `0x806d41`): the spellbook's flush when a record of the spell matched (`0x80476d`), then `PET_BAR_UPDATE_COOLDOWN` always (`0x804792`, `0x806d46`); the order's line shows first (`0x5d4e3c`).
#[test]
fn a_refused_pet_order_or_cast_flushes_the_cooldowns_from_its_handler() {
    use benilla_protocol::messages::{PetActionEntry, PetSpells};
    const GROWL: u32 = 2649;
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    let mut spells = PetSpells {
        pet_guid: PET,
        state: 0x0001,
        ..Default::default()
    };
    spells.bar[3] = PetActionEntry::from(0xC100_0000 | GROWL);
    rig.packets(
        vec![
            hunter_pet(1_000_000, 4200),
            self_values(&[
                (FIELD_UNIT_SUMMON, PET as u32),
                (FIELD_UNIT_SUMMON + 1, (PET >> 32) as u32),
            ]),
            SessionEvent::PetSpells(Box::new(spells)),
            SessionEvent::SpellCooldowns {
                caster: PET,
                flags: 0,
                cooldowns: vec![(GROWL, 30_000)],
            },
        ],
        3,
    );
    let events = &[
        "UI_ERROR_MESSAGE",
        "ACTIONBAR_UPDATE_COOLDOWN",
        "SPELL_UPDATE_COOLDOWN",
        "PET_BAR_UPDATE_COOLDOWN",
    ];
    let verbs = &["petcd"];
    let feedback = |reason, spell| SessionEvent::PetActionFeedback { reason, spell };
    // Growl holds a record: the line, the spellbook's flush, the pet bar's.
    let seen = rig.packets(vec![feedback(2, GROWL)], 2);
    assert_eq!(
        pin(&seen, events, verbs),
        vec![
            "UI_ERROR_MESSAGE There is nothing to attack. | petcd=true",
            "ACTIONBAR_UPDATE_COOLDOWN | petcd=true",
            "SPELL_UPDATE_COOLDOWN | petcd=true",
            "PET_BAR_UPDATE_COOLDOWN | petcd=true",
        ]
    );
    // A spell with no record: the pet bar's flush alone; reason 1 refunds nothing.
    let seen = rig.packets(vec![feedback(3, 1), feedback(1, 0)], 2);
    assert_eq!(
        pin(&seen, events, verbs),
        vec![
            "UI_ERROR_MESSAGE You cannot attack that target. | petcd=true",
            "PET_BAR_UPDATE_COOLDOWN | petcd=true",
            "UI_ERROR_MESSAGE Your pet is dead. | petcd=true",
        ]
    );
    // The cast refusal's refund flushes before its failure line.
    let failed = |spell_id| SessionEvent::PetCastFailed {
        cast_count: 1,
        spell_id,
        reason: 0x0c,
        arg: None,
        wire_reason: 0x0c,
    };
    let seen = rig.packets(vec![failed(GROWL), failed(1)], 2);
    let flushes: Vec<_> = pin(&seen, events, verbs)
        .into_iter()
        .filter(|l| !l.starts_with("UI_ERROR_MESSAGE"))
        .collect();
    assert_eq!(
        flushes,
        vec![
            "ACTIONBAR_UPDATE_COOLDOWN | petcd=true",
            "SPELL_UPDATE_COOLDOWN | petcd=true",
            "PET_BAR_UPDATE_COOLDOWN | petcd=true",
            "PET_BAR_UPDATE_COOLDOWN | petcd=true",
        ]
    );
}

/// A pet created already fighting fires no `PET_ATTACK_START`: a new object's create runs no field handler (`0x4d64a4`, only a re-enabled one reaches `0x4d64be`), and the setter takes the pet's combat test into the attacking global instead (`0x5d6a80`).
#[test]
fn a_pet_created_in_combat_fires_no_attack_start_and_reads_attacking() {
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    let mut pet = hunter_pet(1_000_000, 4200);
    if let SessionEvent::ObjectCreate { fields, .. } = &mut pet {
        fields.merge(ObjectFields::from_pairs(&[(FIELD_UNIT_FLAGS, 0x800)]));
    }
    let events = &["PET_ATTACK_START", "PET_ATTACK_STOP", "PET_BAR_UPDATE"];
    let seen = rig.packets(
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
    assert_eq!(
        pin(&seen, events, &["petattack"]),
        vec!["PET_BAR_UPDATE | petattack=true"]
    );
}

/// `COMPANION_UPDATE "CRITTER"` fires from our `UNIT_FIELD_CRITTER` watch after the field applies (`0x6cf722`), so the critter's active flag reads the new critter at it.
#[test]
fn the_critter_update_fires_from_our_critter_field() {
    use benilla_protocol::field::FIELD_UNIT_CRITTER;
    let Some(mut rig) = Rig::login_after(vec![book(&[CAT])], &[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    rig.packets(vec![creature_answer(7385, "Bombay Cat")], 3);
    // A Bombay Cat's guid: the creature entry in bits 24..51 (`0x802dd8`).
    let critter = 0xF130_0000_0000_0001 | (7385u64 << 24);
    let events = &["COMPANION_UPDATE"];
    let verbs = &["critters", "critter1on"];
    let seen = rig.packets(
        vec![
            self_values(&[
                (FIELD_UNIT_CRITTER, critter as u32),
                (FIELD_UNIT_CRITTER + 1, (critter >> 32) as u32),
            ]),
            self_values(&[(FIELD_UNIT_CRITTER, 0), (FIELD_UNIT_CRITTER + 1, 0)]),
        ],
        3,
    );
    assert_eq!(
        pin(&seen, events, verbs),
        vec![
            "COMPANION_UPDATE CRITTER | critters=1 critter1on=1",
            "COMPANION_UPDATE CRITTER | critters=1 critter1on=nil",
        ]
    );
}

/// `UPDATE_BONUS_ACTIONBAR` fires from our field handler once the possess spell changes: our charm's move looks it up in our auras (`0x6d1ab1`), a far sight on a held subject looks again and finds the same (`0x6e5034`), and one with no held subject clears it (`0x6e5042`); the possess verbs read the held spell at each.
#[test]
fn the_possess_spell_follows_our_charm_and_far_sight_in_the_handler() {
    use benilla_protocol::field::{FIELD_PLAYER_FARSIGHT, FIELD_UNIT_CHARM};
    const MIND_CONTROL: u32 = 605;
    const VICTIM: u64 = 0xF130_0000_0000_0077;
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    let victim = SessionEvent::ObjectCreate {
        guid: VICTIM,
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
        spawned: false,
        fields: ObjectFields::from_pairs(&[(2, 0x9), (FIELD_UNIT_HEALTH, 50)])
            .into_created(ObjectType::Unit),
    };
    let guid =
        |index: u16, g: u64| self_values(&[(index, g as u32), (index + 1, (g >> 32) as u32)]);
    let events = &["UPDATE_BONUS_ACTIONBAR"];
    let verbs = &["possess", "possname"];
    let seen = rig.packets(
        vec![
            victim,
            own_aura(0, MIND_CONTROL),
            guid(FIELD_UNIT_CHARM, VICTIM),
            guid(FIELD_PLAYER_FARSIGHT, VICTIM),
            guid(FIELD_PLAYER_FARSIGHT, 0),
        ],
        3,
    );
    assert_eq!(
        pin(&seen, events, verbs),
        vec![
            "UPDATE_BONUS_ACTIONBAR | possess=true possname=Mind Control",
            "UPDATE_BONUS_ACTIONBAR | possess=false possname=nil",
        ]
    );
    // A far sight on a subject we do not hold misses its lookup (`0x6e5020`) and clears too (`0x6e5042`), after one on the held victim found the spell again (`0x6e5034`).
    const UNHELD: u64 = 0xF130_0000_0000_0099;
    let unheld = rig.packets(
        vec![
            guid(FIELD_PLAYER_FARSIGHT, VICTIM),
            guid(FIELD_PLAYER_FARSIGHT, UNHELD),
        ],
        3,
    );
    assert_eq!(
        pin(&unheld, events, verbs),
        vec![
            "UPDATE_BONUS_ACTIONBAR | possess=true possname=Mind Control",
            "UPDATE_BONUS_ACTIONBAR | possess=false possname=nil",
        ]
    );
}

/// A `/reload` with our charm and its aura held looks the possess spell up after `PLAYER_ENTERING_WORLD` and before the reputation's `UPDATE_FACTION` (`0x52ae84` -> `0x5281a6`, `0x5281fe` -> `0x5d6ede`, `0x528208`), so the bonus bar's handler runs after the UI's world entry; at the login create the aura had not landed, so that entry's lookup found nothing.
#[test]
fn the_world_entry_looks_the_possess_spell_up_after_entering_the_world() {
    use benilla_protocol::field::FIELD_UNIT_CHARM;
    const MIND_CONTROL: u32 = 605;
    const VICTIM: u64 = 0xF130_0000_0000_0077;
    let Some(mut rig) = Rig::login_around(
        Vec::new(),
        &[
            OBJECT_TYPE_PLAYER,
            (FIELD_UNIT_CHARM, VICTIM as u32),
            (FIELD_UNIT_CHARM + 1, (VICTIM >> 32) as u32),
        ],
        vec![own_aura(0, MIND_CONTROL)],
    ) else {
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
                "PLAYER_ENTERING_WORLD",
                "UNIT_PET",
                "PET_BAR_UPDATE",
                "UPDATE_BONUS_ACTIONBAR",
                "UPDATE_FACTION"
            ],
            &["possess", "possname"]
        ),
        vec![
            "UPDATE_FACTION | possess=false possname=nil",
            "PLAYER_ENTERING_WORLD | possess=false possname=nil",
            "PET_BAR_UPDATE | possess=false possname=nil",
            "UPDATE_BONUS_ACTIONBAR | possess=true possname=Mind Control",
            "UPDATE_FACTION | possess=true possname=Mind Control",
        ]
    );
}

/// The UI's entry runs the pet setter with no pet after `PLAYER_ENTERING_WORLD` (`0x5281fe` -> `0x5d6e6d`), ahead of the ask's gate: a bar that beat our create still reads at the entry's event, then the setter's `UNIT_PET` and `PET_BAR_UPDATE` drop it, before `UPDATE_FACTION`; the pet book keeps the packet's count throughout (`0x53dd34`).
#[test]
fn the_world_entry_runs_the_pet_setter_after_entering_the_world() {
    let mut spells = benilla_protocol::messages::PetSpells {
        pet_guid: PET,
        ..Default::default()
    };
    // Growl, a book spell, as its packed word.
    spells.spells = vec![benilla_protocol::messages::PetActionEntry::from(
        0xC100_0000 | 2649,
    )];
    let Some((_rig, seen)) = ported(vec![SessionEvent::PetSpells(Box::new(spells))], Vec::new())
    else {
        return;
    };
    let entry: Vec<String> = pin(
        &seen,
        &[
            "PLAYER_ENTERING_WORLD",
            "UNIT_PET",
            "PET_BAR_UPDATE",
            "UPDATE_FACTION",
        ],
        &["petbar", "petbook"],
    )
    .into_iter()
    .skip_while(|l| !l.starts_with("PLAYER_ENTERING_WORLD"))
    .collect();
    assert_eq!(
        entry,
        vec![
            "PLAYER_ENTERING_WORLD | petbar=1 petbook=1",
            "UNIT_PET player | petbar=nil petbook=1",
            "PET_BAR_UPDATE | petbar=nil petbook=1",
            "UPDATE_FACTION | petbar=nil petbook=1",
        ]
    );
}

/// Logged in, ported to another map and our create sent again in one frame with `before` ahead of it and `after` behind it: the in-game UI is up at the create, as 12340's always is; what the observer saw from that frame on.
fn ported(before: Vec<SessionEvent>, after: Vec<SessionEvent>) -> Option<(Rig, Vec<Seen>)> {
    let mut rig = Rig::login(&[OBJECT_TYPE_PLAYER])?;
    rig.packets(
        vec![SessionEvent::Worldport {
            map_id: 0,
            position: [-8949.95, -132.493, 83.5312],
            orientation: 0.0,
            needs_ack: true,
        }],
        3,
    );
    let frame = before
        .into_iter()
        .chain([self_create(&[OBJECT_TYPE_PLAYER])])
        .chain(after)
        .collect();
    let seen = rig.packets(frame, 4);
    Some((rig, seen))
}

/// A worldport's create with the UI up runs the UI's entry before the packet behind it, as 12340's create calls it inside (`0x6e8186` -> `0x528010`): `PLAYER_ENTERING_WORLD`, the setter's clear and its `PET_BAR_UPDATE` (`0x5d6e6d`), `UPDATE_FACTION` (`0x528208`), then that `SMSG_PET_SPELLS`'s `UNIT_PET` (`0x5d6aca`).
#[test]
fn a_pet_bar_set_behind_our_create_lands_after_the_entry() {
    let Some((_rig, seen)) = ported(
        Vec::new(),
        vec![SessionEvent::PetSpells(Box::new(
            benilla_protocol::messages::PetSpells {
                pet_guid: PET,
                ..Default::default()
            },
        ))],
    ) else {
        return;
    };
    let entry: Vec<String> = pin(
        &seen,
        &[
            "PLAYER_ENTERING_WORLD",
            "UNIT_PET",
            "PET_BAR_UPDATE",
            "UPDATE_FACTION",
        ],
        &["petbar"],
    )
    .into_iter()
    .skip_while(|l| !l.starts_with("PLAYER_ENTERING_WORLD"))
    .collect();
    assert_eq!(
        entry,
        vec![
            "PLAYER_ENTERING_WORLD | petbar=nil",
            "PET_BAR_UPDATE | petbar=nil",
            "UPDATE_FACTION | petbar=nil",
            "UNIT_PET player | petbar=1",
            "PET_BAR_UPDATE | petbar=1",
        ]
    );
}

/// With no pet at all the setter's tail still runs at the entry: no `UNIT_PET`, the temporary bar's `PET_BAR_UPDATE` (`0x5d6a34` -> `0x5d6ad2` -> `0x5d4c1b`).
#[test]
fn the_world_entry_signals_the_pet_bar_with_no_pet() {
    let Some((_rig, seen)) = ported(Vec::new(), Vec::new()) else {
        return;
    };
    let entry: Vec<String> = pin(
        &seen,
        &[
            "PLAYER_ENTERING_WORLD",
            "UNIT_PET",
            "PET_BAR_UPDATE",
            "UPDATE_FACTION",
        ],
        &["petbar"],
    )
    .into_iter()
    .skip_while(|l| !l.starts_with("PLAYER_ENTERING_WORLD"))
    .collect();
    assert_eq!(
        entry,
        vec![
            "PLAYER_ENTERING_WORLD | petbar=nil",
            "PET_BAR_UPDATE | petbar=nil",
            "UPDATE_FACTION | petbar=nil",
        ]
    );
}

/// The setter closes the pet UI for a new pet that can gain experience (`0x5d6a85` -> `0x71b630`, `0x5d6a94`), before its `UNIT_PET`: never on a re-send, for a pet with no pet number, or for one whose creator is not held.
#[test]
fn the_pet_setter_closes_the_pet_ui_for_a_new_hunter_pet() {
    use benilla_protocol::field::{FIELD_UNIT_BYTES_0, FIELD_UNIT_CREATEDBY, FIELD_UNIT_PETNUMBER};
    const MINION: u64 = 0xF140_0000_0100_0002;
    const STRAY: u64 = 0xF140_0000_0100_0003;
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER, (FIELD_UNIT_BYTES_0, 1 | 3 << 8)]) else {
        return;
    };
    let create = |guid: u64, pairs: &[(u16, u32)]| {
        let mut fields = vec![
            (2, 0x9),
            (FIELD_UNIT_HEALTH, 500),
            (FIELD_UNIT_MAXHEALTH, 500),
        ];
        fields.extend_from_slice(pairs);
        SessionEvent::ObjectCreate {
            guid,
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
            fields: ObjectFields::from_pairs(&fields).into_created(ObjectType::Unit),
        }
    };
    let spells = |guid: u64| {
        SessionEvent::PetSpells(Box::new(benilla_protocol::messages::PetSpells {
            pet_guid: guid,
            ..Default::default()
        }))
    };
    let events = &["PET_UI_CLOSE", "UNIT_PET", "PET_BAR_UPDATE"];
    let lines = |seen: &[Seen]| pin(seen, events, &[]);
    let hunter = rig.packets(vec![hunter_pet(1_000_000, 0), spells(PET)], 3);
    let again = rig.packets(vec![spells(PET)], 3);
    let minion = rig.packets(
        vec![
            create(
                MINION,
                &[
                    (FIELD_UNIT_CREATEDBY, ME as u32),
                    (FIELD_UNIT_CREATEDBY + 1, 0),
                ],
            ),
            spells(MINION),
        ],
        3,
    );
    let stray = rig.packets(
        vec![
            create(
                STRAY,
                &[
                    (FIELD_UNIT_PETNUMBER, 7),
                    (FIELD_UNIT_CREATEDBY, 0x99),
                    (FIELD_UNIT_CREATEDBY + 1, 0),
                ],
            ),
            spells(STRAY),
        ],
        3,
    );
    assert_eq!(
        lines(&hunter),
        vec!["PET_UI_CLOSE", "UNIT_PET player", "PET_BAR_UPDATE"]
    );
    assert_eq!(
        lines(&again),
        vec!["PET_BAR_UPDATE"],
        "a re-send closes nothing"
    );
    assert_eq!(
        lines(&minion),
        vec!["UNIT_PET player", "PET_BAR_UPDATE"],
        "no pet number"
    );
    assert_eq!(
        lines(&stray),
        vec!["UNIT_PET player", "PET_BAR_UPDATE"],
        "an unheld creator"
    );
}

/// `GetCompanionCooldown` (`0x53e490`) reads the cooldown history a `SMSG_SPELL_COOLDOWN` of an earlier frame wrote, both from a handler's event and between frames.
#[test]
fn the_companion_cooldown_reads_the_history_an_earlier_frame_wrote() {
    let Some(mut rig) = Rig::login_after(vec![book(&[HORSE])], &[(2, 0x19)]) else {
        return;
    };
    rig.packets(
        vec![
            creature_answer(284, "Brown Horse"),
            SessionEvent::SpellCooldowns {
                caster: ME,
                flags: 0,
                cooldowns: vec![(HORSE, 30_000)],
            },
        ],
        3,
    );
    let between = rig.live_now();
    assert_eq!(between.verbs["mount1cd"], "30");
    let seen = rig.packets(vec![own_aura(3, HORSE)], 3);
    assert_eq!(
        pin(&seen, &["COMPANION_UPDATE"], &["mount1cd"]),
        vec!["COMPANION_UPDATE MOUNT | mount1cd=30"]
    );
}

/// A `SMSG_SPELL_COOLDOWN` and the mount's aura in one frame: the aura handler's `COMPANION_UPDATE` reads the history the cooldown packet just wrote, as `GetCompanionCooldown` asks it at the call (`0x53e54c`), not the frame before's.
#[test]
fn the_companion_cooldown_reads_the_history_the_same_frame_wrote() {
    let Some(mut rig) = Rig::login_after(vec![book(&[HORSE])], &[(2, 0x19)]) else {
        return;
    };
    rig.packets(vec![creature_answer(284, "Brown Horse")], 3);
    assert_eq!(rig.live_now().verbs["mount1cd"], "0");
    let seen = rig.packets(
        vec![
            SessionEvent::SpellCooldowns {
                caster: ME,
                flags: 0,
                cooldowns: vec![(HORSE, 30_000)],
            },
            own_aura(3, HORSE),
        ],
        3,
    );
    assert_eq!(
        pin(&seen, &["COMPANION_UPDATE"], &["mount1cd"]),
        vec!["COMPANION_UPDATE MOUNT | mount1cd=30"]
    );
}

/// The stable master guid the stable scenarios list.
const STABLE_MASTER: u64 = 0xF130_0000_0000_0042;

/// A creature record of `pet_family`, as the stable verbs read it.
fn family_answer(entry: u32, pet_family: u32) -> SessionEvent {
    let SessionEvent::CreatureName {
        entry,
        name,
        subname,
        cursor_name,
        creature_type,
        rank,
        type_flags,
        display_id,
        civilian,
        racial_leader,
        modifiers,
        movement_id,
        ..
    } = creature_answer(entry, "Wolf")
    else {
        unreachable!()
    };
    SessionEvent::CreatureName {
        entry,
        name,
        subname,
        cursor_name,
        creature_type,
        pet_family,
        rank,
        type_flags,
        display_id,
        civilian,
        racial_leader,
        modifiers,
        movement_id,
    }
}

/// The stable master beside us, so the interact target's watchdog keeps the stable open.
fn stable_master_create() -> SessionEvent {
    stable_master_at(STABLE_MASTER)
}

/// A stable master `guid` beside us.
fn stable_master_at(guid: u64) -> SessionEvent {
    SessionEvent::ObjectCreate {
        guid,
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
            (
                benilla_protocol::field::FIELD_UNIT_NPC_FLAGS,
                crate::target::cursor_mode::npc_flags::STABLEMASTER,
            ),
        ])
        .into_created(ObjectType::Unit),
    }
}

/// `MSG_LIST_STABLED_PETS`: Rex out on entry 299, Bruiser stabled on entry 300, two slots bought.
fn stable_list() -> SessionEvent {
    use benilla_protocol::messages::StabledPet;
    SessionEvent::ListStabledPets {
        npc: STABLE_MASTER,
        num_stable_slots: 2,
        pets: vec![
            StabledPet {
                pet_number: 1,
                creature_entry: 299,
                level: 70,
                name: "Rex".into(),
                flags: 1,
            },
            StabledPet {
                pet_number: 2,
                creature_entry: 300,
                level: 60,
                name: "Bruiser".into(),
                flags: 3,
            },
        ],
    }
}

/// A frame logging each stable event with the first stabled pet's family as `GetStablePetInfo` answers it inside that event.
const STABLE_LOG: &str = r#"
STABLE_LOG = {}
local f = CreateFrame("Frame")
for _, e in ipairs({ "PET_STABLE_SHOW", "PET_STABLE_UPDATE", "PET_STABLE_UPDATE_PAPERDOLL", "PET_STABLE_CLOSED" }) do
    f:RegisterEvent(e)
end
f:SetScript("OnEvent", function(_, e)
    STABLE_LOG[#STABLE_LOG + 1] = e .. " " .. tostring((select(4, GetStablePetInfo(1))))
end)
"#;

/// Run `lua` with the `World` lent, as a script call between frames.
fn lua(rig: &mut Rig, lua: &str) -> Vec<String> {
    super::live::with_live_world(rig.app.world_mut(), |s| {
        s.run(lua).unwrap();
        s.eval::<Vec<String>>("local t = STABLE_LOG or {} STABLE_LOG = {} return t")
            .unwrap()
    })
    .expect("a VM")
}

/// The stable list's handler stores the globals and fires `PET_STABLE_SHOW` (`0x5a131a`); its verbs read the globals when called. A record missed by `GetStablePetInfo` fires `PET_STABLE_UPDATE` from its answer (`0x5a0e80`), once per ask, with the record read; a record cached before the list answers inside `PET_STABLE_SHOW`, with no update. Result 11 fires the update from its handler (`0x5a1904`).
#[test]
fn stable_events_fire_from_the_list_and_the_record_answer_in_wire_order() {
    let Some(mut rig) = Rig::login_around(
        Vec::new(),
        &[OBJECT_TYPE_PLAYER],
        vec![stable_master_create()],
    ) else {
        return;
    };
    lua(&mut rig, STABLE_LOG);
    let events = &["PET_STABLE_SHOW", "PET_STABLE_UPDATE", "PET_STABLE_CLOSED"];
    let verbs = &["atstable", "stabled", "stablesel"];
    // The list, then the record's answer, in one frame.
    let seen = rig.packets(vec![stable_list(), family_answer(300, 1)], 3);
    let log = lua(&mut rig, "");
    let shows = pin(&seen, events, verbs);
    assert_eq!(
        shows.first().map(String::as_str),
        Some("PET_STABLE_SHOW | atstable=true stabled=1 stablesel=-1")
    );
    assert_eq!(log.first().map(String::as_str), Some("PET_STABLE_SHOW nil"));
    assert!(
        shows[1..]
            .iter()
            .all(|l| l == "PET_STABLE_UPDATE | atstable=true stabled=1 stablesel=1"),
        "{shows:#?}"
    );
    assert!(shows.len() > 1, "the answer runs the asks: {shows:#?}");
    assert!(
        log[1..].iter().all(|l| l == "PET_STABLE_UPDATE Wolf"),
        "{log:#?}"
    );

    // The record cached before the next list: the show reads it, and no update follows; the stock window's re-pick (`PetStable.lua:48-63`) finds the family and selects before we read.
    let seen = rig.packets(vec![stable_list()], 3);
    assert_eq!(
        pin(&seen, events, verbs),
        vec!["PET_STABLE_SHOW | atstable=true stabled=1 stablesel=1"]
    );
    assert_eq!(lua(&mut rig, ""), vec!["PET_STABLE_SHOW Wolf"]);

    // Result 11.
    let seen = rig.packets(vec![SessionEvent::StableResult { result: 11 }], 2);
    assert_eq!(
        pin(&seen, events, &["atstable"]),
        vec!["PET_STABLE_UPDATE | atstable=true"]
    );
}

/// `ClosePetStables` clears the master and fires `PET_STABLE_CLOSED` inside the call, so the next verb in the same chunk reads it gone, while the list stays (`0x5a1996`, `0x5a19aa`); a second close fires nothing. The master is the interact target the list set (`0x5a11d9`), so its clear runs the interaction close's stable arm first, which fires too (`0x5a198a`, `0x513065`, `0x5a0f4c`). The connection's end leaves the world, closing a held stable the same two ways (`0x528d5a`, `0x5a1174`, `0x5a1194`).
#[test]
fn the_stable_close_fires_inside_the_call_and_the_session_end_closes() {
    let Some(mut rig) = Rig::login_around(
        Vec::new(),
        &[OBJECT_TYPE_PLAYER],
        vec![stable_master_create()],
    ) else {
        return;
    };
    lua(&mut rig, STABLE_LOG);
    rig.packets(vec![family_answer(300, 1), stable_list()], 3);
    lua(&mut rig, "");
    let after = lua(
        &mut rig,
        "ClosePetStables() STABLE_LOG[#STABLE_LOG + 1] = 'after ' .. tostring(IsAtStableMaster()) .. ' ' .. GetNumStablePets() ClosePetStables()",
    );
    assert_eq!(
        after,
        vec![
            "PET_STABLE_CLOSED Wolf",
            "PET_STABLE_CLOSED Wolf",
            "after false 1"
        ]
    );

    rig.packets(vec![stable_list()], 3);
    lua(&mut rig, "");
    let seen = rig.packets(
        vec![SessionEvent::Disconnected {
            reason: "socket".into(),
            end: benilla_protocol::SessionEnd::Lost,
        }],
        1,
    );
    assert_eq!(
        pin(&seen, &["PET_STABLE_CLOSED"], &["atstable"]),
        vec!["PET_STABLE_CLOSED | atstable=false"; 2]
    );
}

/// A list from another master moves the interact target (`0x5a11d9` -> `0x51fd1c`), whose clear runs the interaction close's stable arm on the held master (`0x518db7` -> `0x512e60` -> `0x513065` -> `0x5a0f10`): `PET_STABLE_CLOSED` (`0x5a0f4c`) before the new list's `PET_STABLE_SHOW` (`0x5a131a`), the close with no master held, so the stock window's hide (`ClosePetStables`) clears nothing. The same master's list closes nothing.
#[test]
fn another_masters_list_closes_the_held_stable_before_its_show() {
    let Some(mut rig) = Rig::login_around(
        Vec::new(),
        &[OBJECT_TYPE_PLAYER],
        vec![stable_master_create(), stable_master_at(STABLE_MASTER + 1)],
    ) else {
        return;
    };
    lua(&mut rig, STABLE_LOG);
    let events = &["PET_STABLE_SHOW", "PET_STABLE_CLOSED"];
    rig.packets(vec![family_answer(300, 1), stable_list()], 3);
    lua(&mut rig, "");
    let seen = rig.packets(vec![stable_list()], 3);
    assert_eq!(
        pin(&seen, events, &["atstable"]),
        vec!["PET_STABLE_SHOW | atstable=true"]
    );
    let SessionEvent::ListStabledPets {
        num_stable_slots,
        pets,
        ..
    } = stable_list()
    else {
        unreachable!()
    };
    let other = SessionEvent::ListStabledPets {
        npc: STABLE_MASTER + 1,
        num_stable_slots,
        pets,
    };
    let seen = rig.packets(vec![other], 3);
    assert_eq!(
        pin(&seen, events, &["atstable"]),
        vec![
            "PET_STABLE_CLOSED | atstable=false",
            "PET_STABLE_SHOW | atstable=true"
        ]
    );
}

/// An open-stable aura's removal closes the stable forced: `PET_STABLE_CLOSED` fires from the aura handler with no master held too (`0x71ea25`); its arrival fires nothing.
#[test]
fn an_open_stable_auras_removal_closes_the_stable() {
    /// Call Stabled Pet, whose aura opens the stable.
    const CALL_STABLED_PET: u32 = 62757;
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    let seen = rig.packets(vec![own_aura(4, CALL_STABLED_PET), own_aura(4, 0)], 2);
    assert_eq!(
        pin(&seen, &["PET_STABLE_CLOSED"], &["atstable"]),
        vec!["PET_STABLE_CLOSED | atstable=false"]
    );
}

/// A supersede with our object held runs the removal's companion arm, then the registrar's with notify set (`0x542eba`, `0x542ec6`): the old mount leaves, the new critter joins and is learned.
#[test]
fn a_supersede_unlearns_the_old_companion_and_learns_the_new() {
    let Some(mut rig) = Rig::login_after(vec![book(&[HORSE])], &[(2, 0x19)]) else {
        return;
    };
    rig.packets(
        vec![
            creature_answer(284, "Brown Horse"),
            creature_answer(7385, "Bombay Cat"),
        ],
        3,
    );
    let seen = rig.packets(
        vec![SessionEvent::SpellSuperceded {
            old_spell_id: HORSE,
            new_spell_id: CAT,
        }],
        3,
    );
    assert_eq!(
        pin(
            &seen,
            &[
                "COMPANION_UPDATE",
                "COMPANION_LEARNED",
                "COMPANION_UNLEARNED"
            ],
            &["mounts", "critters"]
        ),
        // The emptied lists sort nothing, so the removal fires no update; the handler's events fire once its changes apply, so each reads both lists moved.
        vec![
            "COMPANION_UNLEARNED | mounts=0 critters=1",
            "COMPANION_UPDATE | mounts=0 critters=1",
            "COMPANION_LEARNED | mounts=0 critters=1",
        ]
    );
}

// ── Runes and totems (#1281) ─────────────────────────────────────────────────────────────────────

use benilla_protocol::field::FIELD_PLAYER_RUNE_REGEN_1;

/// A death knight's rates on our create: ten seconds a rune for every kind.
fn rune_rates() -> Vec<(u16, u32)> {
    let mut f = vec![OBJECT_TYPE_PLAYER];
    f.extend((0..4).map(|k| (FIELD_PLAYER_RUNE_REGEN_1 + k, 0.1f32.to_bits())));
    f
}

/// The rune packets apply in their handlers and fire there, in wire order: the resync's six
/// `RUNE_POWER_UPDATE`s each read the whole resync applied (`0x5ee440`), the convert's
/// `RUNE_TYPE_UPDATE` the new kind (`0x5ee020`), and the power add's `ACTIONBAR_UPDATE_COOLDOWN`
/// comes last (`0x5ee520`). The cooldown's duration is the player's rate read at the call
/// (`0x613098`).
#[test]
fn rune_packets_fire_from_their_handlers_in_wire_order() {
    let Some(mut rig) = Rig::login(&rune_rates()) else {
        return;
    };
    let seen = rig.packets(
        vec![
            SessionEvent::ResyncRunes(benilla_protocol::messages::ResyncRunes {
                count: 6,
                runes: vec![(0, 255), (0, 0), (1, 255), (1, 255), (2, 255), (2, 255)],
            }),
            SessionEvent::ConvertRune(benilla_protocol::messages::ConvertRune { rune: 1, kind: 3 }),
            SessionEvent::AddRunePower { mask: 0x2 },
        ],
        2,
    );
    let events = &[
        "RUNE_POWER_UPDATE",
        "RUNE_TYPE_UPDATE",
        "ACTIONBAR_UPDATE_COOLDOWN",
    ];
    let verbs = &["rune2", "rune2dur", "rune2ready"];
    assert_eq!(
        pin(&seen, events, verbs),
        vec![
            "RUNE_POWER_UPDATE 1 false | rune2=1 rune2dur=10 rune2ready=false",
            "RUNE_POWER_UPDATE 2 false | rune2=1 rune2dur=10 rune2ready=false",
            "RUNE_POWER_UPDATE 3 false | rune2=1 rune2dur=10 rune2ready=false",
            "RUNE_POWER_UPDATE 4 false | rune2=1 rune2dur=10 rune2ready=false",
            "RUNE_POWER_UPDATE 5 false | rune2=1 rune2dur=10 rune2ready=false",
            "RUNE_POWER_UPDATE 6 false | rune2=1 rune2dur=10 rune2ready=false",
            "RUNE_TYPE_UPDATE 2 | rune2=4 rune2dur=10 rune2ready=false",
            "ACTIONBAR_UPDATE_COOLDOWN | rune2=4 rune2dur=10 rune2ready=false",
        ]
    );
}

/// The world entry resets the rune table after `PLAYER_ENTERING_WORLD` (`0x5281a6`, then
/// `0x528294` -> `0x5edef0`), so the entry's handlers read the table the packets left, and the
/// verbs after it the game start's.
#[test]
fn the_world_entry_resets_the_runes_after_entering_the_world() {
    let Some(mut rig) = Rig::login(&rune_rates()) else {
        return;
    };
    let convert =
        SessionEvent::ConvertRune(benilla_protocol::messages::ConvertRune { rune: 1, kind: 3 });
    rig.packets(vec![convert, SessionEvent::ObjectDestroyed(ME)], 2);
    let seen = rig.packets(vec![self_create(&rune_rates())], 3);
    assert_eq!(
        pin(&seen, &["PLAYER_ENTERING_WORLD"], &["rune2"]),
        vec!["PLAYER_ENTERING_WORLD | rune2=4"]
    );
    assert_eq!(rig.live_now().line(&["rune2"]), "NOW | rune2=1");
}

/// The totem guid.
const TOTEM: u64 = 0xF130_0000_0000_0050;

fn totem_unit() -> SessionEvent {
    SessionEvent::ObjectCreate {
        guid: TOTEM,
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
            (FIELD_UNIT_HEALTH, 5),
            (FIELD_UNIT_MAXHEALTH, 5),
            (FIELD_UNIT_LEVEL, 80),
        ])
        .into_created(ObjectType::Unit),
    }
}

/// `SMSG_TOTEM_CREATED` stores its slot silently (`0x526d4c`); the totem unit's create announces
/// it (`0x740140`), and its destroy's disable sends, clears and announces it (`0x73574a`), each
/// in its handler's turn. The verbs read the table the handler left.
#[test]
fn totem_events_fire_from_the_unit_create_and_disable() {
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    let created = SessionEvent::TotemCreated(benilla_protocol::messages::TotemCreated {
        slot: 0,
        guid: TOTEM,
        duration_ms: 60_000,
        spell_id: 3599,
    });
    let summoned = rig.packets(vec![created, totem_unit()], 2);
    let gone = rig.packets(vec![SessionEvent::ObjectDestroyed(TOTEM)], 2);
    assert_eq!(
        pin(
            &summoned,
            &["PLAYER_TOTEM_UPDATE"],
            &["totem1", "totem1name"]
        ),
        vec!["PLAYER_TOTEM_UPDATE 1 | totem1=true totem1name=nil"]
    );
    let verbs = &["totem1", "totem1left"];
    assert_eq!(
        pin(&gone, &["PLAYER_TOTEM_UPDATE"], verbs),
        vec!["PLAYER_TOTEM_UPDATE 1 | totem1=false totem1left=0"]
    );
}

/// `DestroyTotem` sends, clears and announces inside the call (`0x51d562` -> `0x5197d0`): the
/// same chunk reads the slot empty, and the event has fired before the verb returned.
#[test]
fn destroy_totem_acts_inside_the_call() {
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    let created = SessionEvent::TotemCreated(benilla_protocol::messages::TotemCreated {
        slot: 2,
        guid: TOTEM,
        duration_ms: 60_000,
        spell_id: 3599,
    });
    rig.packets(vec![created], 2);
    let answers = super::live::with_live_world(rig.app.world_mut(), |s| {
        s.eval::<(bool, usize, bool)>(
            "local before = (GetTotemInfo(3)) DestroyTotem(3) \
             local fired = 0 for _, l in ipairs(CHARZ_LOG) do \
               if l:find('^PLAYER_TOTEM_UPDATE') then fired = fired + 1 end end \
             return before, fired, (GetTotemInfo(3))",
        )
    })
    .expect("a VM")
    .expect("the chunk runs");
    assert_eq!(answers, (true, 1, false));
}

/// Our attack start and stop fire `PLAYER_ENTER_COMBAT` and `PLAYER_LEAVE_COMBAT` from their
/// handlers (`0x756897`, `0x756b86`), so a flags update between them fires its `UNIT_FLAGS`
/// between them, in wire order. `PLAYER_REGEN_DISABLED` still fires from the units' feed after the
/// frame's packets (#1289).
#[test]
fn attack_start_and_stop_fire_from_their_handlers_in_wire_order() {
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    const MOB: u64 = 0xF130_0000_0000_0099;
    let seen = rig.packets(
        vec![
            SessionEvent::AttackStart {
                attacker: ME,
                victim: MOB,
            },
            flags(crate::player::UNIT_FLAG_IN_COMBAT),
            SessionEvent::AttackStop {
                attacker: ME,
                victim: MOB,
                face_victim: false,
            },
        ],
        3,
    );
    let events = &[
        "PLAYER_ENTER_COMBAT",
        "PLAYER_LEAVE_COMBAT",
        "UNIT_FLAGS",
        "PLAYER_REGEN_DISABLED",
    ];
    assert_eq!(
        pin(&seen, events, &["combat"]),
        vec![
            "PLAYER_ENTER_COMBAT | combat=nil",
            "UNIT_FLAGS player | combat=1",
            "PLAYER_LEAVE_COMBAT | combat=1",
            "PLAYER_REGEN_DISABLED | combat=1",
        ]
    );
}

/// A cast's edges apply in their handlers and fire there, in wire order: `SMSG_SPELL_START` opens
/// our cast and fires `UNIT_SPELLCAST_START` with it already readable (`0x805409`), and
/// `SMSG_SPELL_GO` fires `SUCCEEDED` with the cast still running, then `STOP` with it gone
/// (`0x7fecc0`, `0x7fee38`); a flags update between them fires between them.
#[test]
fn cast_bar_events_fire_from_the_spell_handlers_in_wire_order() {
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    const FIREBALL: u32 = 133;
    let start = SessionEvent::SpellStart {
        caster: ME,
        cast_count: 4,
        spell_id: FIREBALL,
        cast_flags: 0,
        cast_time_ms: 1500,
        target: None,
        ammo_display_id: None,
        immunities: None,
    };
    let go = SessionEvent::SpellGo {
        caster: ME,
        cast_count: 4,
        spell_id: FIREBALL,
        cast_flags: 0,
        server_time: 0,
        hits: vec![],
        misses: vec![],
        reflects: vec![],
        target: None,
        go_target: None,
        target_mask: 0,
        src: None,
        dest: None,
        ammo: None,
        item_caster: None,
        trajectory: None,
        dest_counter: 0,
        points: vec![],
    };
    let seen = rig.packets(
        vec![start, flags(crate::player::UNIT_FLAG_IN_COMBAT), go],
        2,
    );
    let events = &[
        "UNIT_SPELLCAST_START",
        "UNIT_FLAGS",
        "UNIT_SPELLCAST_SUCCEEDED",
        "UNIT_SPELLCAST_STOP",
    ];
    assert_eq!(
        pin(&seen, events, &["casting", "castid"]),
        vec![
            "UNIT_SPELLCAST_START player Fireball Rank 1 4 | casting=Fireball castid=4",
            "UNIT_FLAGS player | casting=Fireball castid=4",
            "UNIT_SPELLCAST_SUCCEEDED player Fireball Rank 1 4 | casting=Fireball castid=4",
            "UNIT_SPELLCAST_STOP player Fireball Rank 1 4 | casting=nil castid=nil",
        ]
    );
}

/// The requirement state is the active player's, read at the call: a level update's own events read the new level's green band (`0x5e28a1`) with no frame between.
#[test]
fn a_level_update_reads_the_new_level_in_its_own_events() {
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER, (FIELD_UNIT_LEVEL, 1)]) else {
        return;
    };
    let seen = rig.packets(vec![self_values(&[(FIELD_UNIT_LEVEL, 40)])], 2);
    assert_eq!(
        pin(&seen, &["UNIT_LEVEL", "PLAYER_LEVEL_UP"], &["greenrange"]),
        vec!["UNIT_LEVEL player | greenrange=8"]
    );
}

/// The currency list is built at 12340's triggers and its events fire from their handlers: the world entry's (`0x528299`) after `UPDATE_FACTION` (`0x528208`); the known-currencies watch's `KNOWN_CURRENCY_TYPES_UPDATE` (`0x6e5401` -> `0x5b0640`) with the row's record asked; that record's landing (`0x5b0260` -> `0x5b0660`); a token slot's (`0x5d991f`) and our token's count (`0x707832`), each read at its own event.
#[test]
fn currency_triggers_build_the_list_and_fire_from_their_handlers() {
    use benilla_protocol::field::{
        FIELD_ITEM_OWNER, FIELD_ITEM_STACK_COUNT, FIELD_OBJECT_ENTRY,
        FIELD_PLAYER_CURRENCY_TOKEN_SLOT_1, FIELD_PLAYER_KNOWN_CURRENCIES,
    };
    const BADGE: u32 = 29434;
    const BADGE_GUID: u64 = 0x4000_0000_0000_0300;
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    let login = fired(
        &rig.take(),
        &[
            "PLAYER_ENTERING_WORLD",
            "UPDATE_FACTION",
            "CURRENCY_DISPLAY_UPDATE",
        ],
    );
    assert_eq!(
        login,
        [
            "UPDATE_FACTION",
            "PLAYER_ENTERING_WORLD",
            "UPDATE_FACTION",
            "CURRENCY_DISPLAY_UPDATE"
        ],
        "the first UPDATE_FACTION is the pre-player faction packet's"
    );
    let bit = rig
        .app
        .world()
        .resource::<crate::ui_currency::Currencies>()
        .0
        .types
        .iter()
        .find(|t| t.item_id == BADGE)
        .map(|t| t.bit_index)
        .expect("the badge's row");
    let known = 1u64 << (bit - 1);
    let events = &["KNOWN_CURRENCY_TYPES_UPDATE", "CURRENCY_DISPLAY_UPDATE"];
    let verbs = &["cursize", "cur2"];
    let seen = rig.packets(
        vec![self_values(&[
            (FIELD_PLAYER_KNOWN_CURRENCIES, known as u32),
            (FIELD_PLAYER_KNOWN_CURRENCIES + 1, (known >> 32) as u32),
        ])],
        2,
    );
    assert_eq!(
        pin(&seen, events, verbs),
        ["KNOWN_CURRENCY_TYPES_UPDATE | cursize=2 cur2=nilx0"],
        "one watch for the pair; the record is not cached yet"
    );
    let mut badge = crate::items::test_template("Badge of Justice");
    badge.class = 10;
    let seen = rig.packets(
        vec![SessionEvent::ItemTemplate {
            entry: BADGE,
            info: Some(Box::new(badge)),
        }],
        2,
    );
    assert_eq!(
        pin(&seen, events, verbs),
        ["CURRENCY_DISPLAY_UPDATE | cursize=2 cur2=Badge of Justicex0"]
    );
    let seen = rig.packets(
        vec![
            SessionEvent::ItemCreate {
                guid: BADGE_GUID,
                container: false,
                fields: ObjectFields::from_pairs(&[
                    (FIELD_OBJECT_ENTRY, BADGE),
                    (FIELD_ITEM_OWNER, ME as u32),
                    (FIELD_ITEM_OWNER + 1, (ME >> 32) as u32),
                    (FIELD_ITEM_STACK_COUNT, 5),
                ])
                .into_created(ObjectType::Item),
            },
            self_values(&[
                (FIELD_PLAYER_CURRENCY_TOKEN_SLOT_1, BADGE_GUID as u32),
                (
                    FIELD_PLAYER_CURRENCY_TOKEN_SLOT_1 + 1,
                    (BADGE_GUID >> 32) as u32,
                ),
            ]),
        ],
        2,
    );
    assert_eq!(
        pin(&seen, events, verbs),
        ["CURRENCY_DISPLAY_UPDATE | cursize=2 cur2=Badge of Justicex5"],
        "one watch for the slot's two words"
    );
    // The token slot's watch updates our contents before the currency leg (`0x5d98e3`, `0x5d991f`).
    assert_eq!(
        fired(&seen, &["BAG_UPDATE", "CURRENCY_DISPLAY_UPDATE"]),
        [
            "BAG_UPDATE",
            "BAG_UPDATE",
            "BAG_UPDATE",
            "CURRENCY_DISPLAY_UPDATE"
        ]
    );
    let seen = rig.packets(
        vec![SessionEvent::ObjectValues {
            guid: BADGE_GUID,
            fields: ObjectFields::from_pairs(&[(FIELD_ITEM_STACK_COUNT, 7)]),
        }],
        2,
    );
    assert_eq!(
        pin(&seen, events, verbs),
        ["CURRENCY_DISPLAY_UPDATE | cursize=2 cur2=Badge of Justicex7"]
    );
    // One update moving a packed token's count and enchants: the count handler's currency leg fires before the enchant handler runs (`0x707832`, `0x708bd0`), each handler's `BAG_UPDATE`s, our backpack, keyring and tokens (`0x5d7098`-`0x5d70ba`), in its turn (`0x707805`, `0x707fe4`).
    const PACKED: u64 = 0x4000_0000_0000_0302;
    let slot2 = benilla_protocol::field::FIELD_PLAYER_PACK_SLOT_1 + 2;
    rig.packets(
        vec![
            SessionEvent::ItemCreate {
                guid: PACKED,
                container: false,
                fields: ObjectFields::from_pairs(&[
                    (FIELD_OBJECT_ENTRY, BADGE),
                    (FIELD_ITEM_OWNER, ME as u32),
                    (FIELD_ITEM_OWNER + 1, (ME >> 32) as u32),
                    (benilla_protocol::field::FIELD_ITEM_CONTAINED, ME as u32),
                    (
                        benilla_protocol::field::FIELD_ITEM_CONTAINED + 1,
                        (ME >> 32) as u32,
                    ),
                    (FIELD_ITEM_STACK_COUNT, 1),
                ])
                .into_created(ObjectType::Item),
            },
            self_values(&[(slot2, PACKED as u32), (slot2 + 1, (PACKED >> 32) as u32)]),
        ],
        2,
    );
    let seen = rig.packets(
        vec![SessionEvent::ObjectValues {
            guid: PACKED,
            fields: ObjectFields::from_pairs(&[
                (FIELD_ITEM_STACK_COUNT, 2),
                (benilla_protocol::field::FIELD_ITEM_ENCHANTMENT_1_1, 1),
            ]),
        }],
        2,
    );
    assert_eq!(
        fired(&seen, &["BAG_UPDATE", "CURRENCY_DISPLAY_UPDATE"]),
        [
            "BAG_UPDATE",
            "BAG_UPDATE",
            "BAG_UPDATE",
            "CURRENCY_DISPLAY_UPDATE",
            "BAG_UPDATE",
            "BAG_UPDATE",
            "BAG_UPDATE"
        ]
    );
    // A pack slot's leg reads the old item's class inside the field handler, before the same frame's destroy of it (`0x5d97fd`).
    const LOOSE: u64 = 0x4000_0000_0000_0301;
    let pack = benilla_protocol::field::FIELD_PLAYER_PACK_SLOT_1;
    rig.packets(
        vec![
            SessionEvent::ItemCreate {
                guid: LOOSE,
                container: false,
                fields: ObjectFields::from_pairs(&[
                    (FIELD_OBJECT_ENTRY, BADGE),
                    (FIELD_ITEM_STACK_COUNT, 1),
                ])
                .into_created(ObjectType::Item),
            },
            self_values(&[(pack, LOOSE as u32), (pack + 1, (LOOSE >> 32) as u32)]),
        ],
        2,
    );
    let seen = rig.packets(
        vec![
            self_values(&[(pack, 0), (pack + 1, 0)]),
            SessionEvent::ObjectDestroyed(LOOSE),
        ],
        2,
    );
    assert_eq!(
        fired(&seen, &["CURRENCY_DISPLAY_UPDATE"]),
        ["CURRENCY_DISPLAY_UPDATE"],
        "the old item is still held when its slot's handler runs"
    );
}

/// The bag watches fire from the update's handler in field order, each read at its own event: a pack slot's watch (`0x5d9790`) unlocks the moved item (`0x5d9863`) before our `BAG_UPDATE`s (`0x5d98e3` -> `0x5d7070`); a bag slot's inventory watch (`0x6dedd0`) runs ahead of the containers' (`0x5d9b40`), whose refresh closes the old bag before the new one's update (`0x5d9a87`, `0x5d9b16`); a vault or bank-bag slot fires its bank slot (`0x6deed6`).
#[test]
fn the_bag_watches_fire_from_the_update_in_field_order() {
    use benilla_protocol::field::{
        FIELD_CONTAINER_NUM_SLOTS, FIELD_ITEM_CONTAINED, FIELD_ITEM_OWNER, FIELD_ITEM_STACK_COUNT,
        FIELD_OBJECT_ENTRY, FIELD_PLAYER_BANK_BAG_SLOT_1, FIELD_PLAYER_BANK_SLOT_1,
        FIELD_PLAYER_INV_SLOT_HEAD, FIELD_PLAYER_PACK_SLOT_1,
    };
    const ARROWS: u64 = 0x4000_0000_0000_0400;
    const POUCH: u64 = 0x4000_0000_0000_0401;
    const SACK: u64 = 0x4000_0000_0000_0402;
    const STONE: u64 = 0x4000_0000_0000_0403;
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    let item = |guid: u64, entry: u32, slots: Option<u32>| {
        let mut pairs = vec![
            (FIELD_OBJECT_ENTRY, entry),
            (FIELD_ITEM_OWNER, ME as u32),
            (FIELD_ITEM_OWNER + 1, (ME >> 32) as u32),
            (FIELD_ITEM_CONTAINED, ME as u32),
            (FIELD_ITEM_CONTAINED + 1, (ME >> 32) as u32),
            (FIELD_ITEM_STACK_COUNT, 20),
        ];
        if let Some(n) = slots {
            pairs.push((FIELD_CONTAINER_NUM_SLOTS, n));
        }
        let kind = if slots.is_some() {
            ObjectType::Container
        } else {
            ObjectType::Item
        };
        SessionEvent::ItemCreate {
            guid,
            container: slots.is_some(),
            fields: ObjectFields::from_pairs(&pairs).into_created(kind),
        }
    };
    let slot = |field: u16, guid: u64| [(field, guid as u32), (field + 1, (guid >> 32) as u32)];
    let events = &[
        "BAG_UPDATE",
        "BAG_CLOSED",
        "ITEM_LOCK_CHANGED",
        "PLAYERBANKSLOTS_CHANGED",
    ];
    let verbs = &["free0", "cnt01", "lock01", "slots1"];

    let seen = rig.packets(
        vec![
            item(ARROWS, 2512, None),
            self_values(&slot(FIELD_PLAYER_PACK_SLOT_1, ARROWS)),
        ],
        2,
    );
    assert_eq!(
        pin(&seen, events, verbs),
        [
            "BAG_UPDATE 0 | free0=15 cnt01=20 lock01=nil slots1=0",
            "BAG_UPDATE -2 | free0=15 cnt01=20 lock01=nil slots1=0",
            "BAG_UPDATE -4 | free0=15 cnt01=20 lock01=nil slots1=0",
        ],
        "a pack slot's watch, read at its event"
    );

    // The move's send locks the item at its one held end (`0x706fe0`).
    super::live::with_live_world(rig.app.world_mut(), |s| {
        s.run("PickupContainerItem(0, 1) PickupContainerItem(0, 2)")
    })
    .expect("a VM")
    .expect("the move runs");
    let seen = rig.packets(Vec::new(), 2);
    let lock_events = &["ITEM_LOCK_CHANGED", "ITEM_LOCKED", "ITEM_UNLOCKED"];
    let locks = pin(&seen, lock_events, &["lock01"]);
    assert_eq!(
        locks[locks.len().saturating_sub(2)..],
        [
            "ITEM_LOCK_CHANGED 0 1 | lock01=1",
            "ITEM_LOCKED 0 1 | lock01=1"
        ],
        "the send's lock reads locked at its events: {locks:?}"
    );
    // Slot 1's watch unlocks its new item, none (`0x5d9863` -> `0x513770`), then our updates; slot 2's unlocks the arrows at their new place, then ours again.
    let mut words = slot(FIELD_PLAYER_PACK_SLOT_1, 0).to_vec();
    words.extend(slot(FIELD_PLAYER_PACK_SLOT_1 + 2, ARROWS));
    let seen = rig.packets(vec![self_values(&words)], 2);
    let moved: Vec<&str> = events.iter().chain(lock_events).copied().collect();
    assert_eq!(
        pin(&seen, &moved, &["free0", "lock02"]),
        [
            "BAG_UPDATE 0 | free0=15 lock02=1",
            "BAG_UPDATE -2 | free0=15 lock02=1",
            "BAG_UPDATE -4 | free0=15 lock02=1",
            "ITEM_LOCK_CHANGED 0 2 | free0=15 lock02=nil",
            "ITEM_UNLOCKED 0 2 | free0=15 lock02=nil",
            "BAG_UPDATE 0 | free0=15 lock02=nil",
            "BAG_UPDATE -2 | free0=15 lock02=nil",
            "BAG_UPDATE -4 | free0=15 lock02=nil",
        ],
        "no event for the emptied slot; the arrows unlock where they landed"
    );

    // A bag in the first bag slot, then another over it.
    let bag_slot = FIELD_PLAYER_INV_SLOT_HEAD + 2 * 19;
    let seen = rig.packets(
        vec![
            item(POUCH, 828, Some(6)),
            self_values(&slot(bag_slot, POUCH)),
        ],
        2,
    );
    assert_eq!(
        pin(&seen, events, &["slots1"]),
        [
            "BAG_UPDATE 0 | slots1=0",
            "BAG_UPDATE -2 | slots1=0",
            "BAG_UPDATE -4 | slots1=0",
            "BAG_UPDATE 1 | slots1=6",
        ],
        "our contents update before the refresh watches the new bag (0x5d9bf9, 0x5d9bfe)"
    );
    let seen = rig.packets(
        vec![
            item(SACK, 4496, Some(8)),
            self_values(&slot(bag_slot, SACK)),
        ],
        2,
    );
    assert_eq!(
        pin(&seen, events, &["slots1"]),
        [
            "BAG_UPDATE 0 | slots1=6",
            "BAG_UPDATE -2 | slots1=6",
            "BAG_UPDATE -4 | slots1=6",
            "BAG_CLOSED 1 | slots1=6",
            "BAG_UPDATE 1 | slots1=8",
        ]
    );

    // The vault's first slot, then the first bank-bag slot.
    let seen = rig.packets(
        vec![
            item(STONE, 6948, None),
            self_values(&slot(FIELD_PLAYER_BANK_SLOT_1, STONE)),
        ],
        2,
    );
    assert_eq!(pin(&seen, events, &[]), ["PLAYERBANKSLOTS_CHANGED 1"]);
    let seen = rig.packets(
        vec![self_values(&slot(FIELD_PLAYER_BANK_BAG_SLOT_1, POUCH))],
        2,
    );
    assert_eq!(
        pin(&seen, events, &[]),
        [
            "PLAYERBANKSLOTS_CHANGED 29",
            "BAG_UPDATE 0",
            "BAG_UPDATE -2",
            "BAG_UPDATE -4",
            "BAG_UPDATE 5",
        ]
    );
}

/// An item set's tooltip block reads its row and the members' names at the call (`0x62ba3e`): an unanswered name is asked and the tooltip waits on it, and the name's answer renders the tooltip again from the handler (`0x62ba29`).
#[test]
fn an_item_sets_block_reads_its_row_and_names_at_the_call() {
    const SET: u32 = 161;
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    let (set_name, members) = {
        let sets = rig.app.world().resource::<crate::ui_items::ItemSets>();
        let row = sets.0.set(SET).expect("ItemSet.dbc's row 161");
        (row.name.clone(), row.items.clone())
    };
    let piece = members[0];
    let mut info = crate::items::test_template("Set Piece");
    info.item_set = SET;
    rig.packets(
        vec![SessionEvent::ItemTemplate {
            entry: piece,
            info: Some(Box::new(info)),
        }],
        2,
    );
    let read = |rig: &mut Rig| -> String {
        rig.app
            .world()
            .non_send_resource::<benilla_ui::script::UiScript>()
            .eval(
                "local t = {} for i = 1, GameTooltip:NumLines() do                  t[#t + 1] = _G['GameTooltipTextLeft' .. i]:GetText() or '' end                  return table.concat(t, '|')",
            )
            .expect("the tooltip's lines")
    };
    super::live::with_live_world(rig.app.world_mut(), |s| {
        s.run(&format!(
            "GameTooltip:SetOwner(UIParent, 'ANCHOR_NONE') GameTooltip:SetHyperlink('item:{piece}')"
        ))
    })
    .expect("a VM")
    .expect("the hover runs");
    let before = read(&mut rig);
    assert!(before.contains(&set_name), "the set's row read: {before}");
    assert!(!before.contains("Second Piece"), "{before}");
    let seen = rig.packets(
        vec![SessionEvent::ItemName {
            entry: members[1],
            name: Some(("Second Piece".into(), 5)),
        }],
        2,
    );
    let _ = seen;
    let after = read(&mut rig);
    assert!(
        after.contains("Second Piece"),
        "the name's answer renders the block again: {after}"
    );
}

/// The inventory watch runs on the worn slots too (`0x6e51c5`-`0x6e51fb`): an equipped item unlocks at its new slot (`0x6dee39`) before the slot's `PLAYER_EQUIPMENT_CHANGED` (`0x6deea2`), an emptied slot fires it with the slot alone (`0x6dee81`); an owned item's entry change drops the old record (`0x7079bb`) and fires its container's `BAG_UPDATE` from the record callback, at once when cached (`0x707a07` -> `0x707949`), else when the record lands.
#[test]
fn a_worn_slot_and_an_entry_change_run_their_watches() {
    use crate::pending_item_ops::PendingItemOps;
    use benilla_protocol::field::{
        FIELD_ITEM_CONTAINED, FIELD_ITEM_OWNER, FIELD_ITEM_STACK_COUNT, FIELD_OBJECT_ENTRY,
        FIELD_PLAYER_INV_SLOT_HEAD, FIELD_PLAYER_PACK_SLOT_1,
    };
    const SWORD: u64 = 0x4000_0000_0000_0600;
    const STONE: u64 = 0x4000_0000_0000_0601;
    const SWORD_ENTRY: u32 = 25;
    const OLD: u32 = 6948;
    const CACHED: u32 = 4496;
    const ASKED: u32 = 2589;
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    rig.packets(
        [
            (SWORD_ENTRY, "Worn Shortsword"),
            (OLD, "Hearthstone"),
            (CACHED, "Small Brown Pouch"),
        ]
        .into_iter()
        .map(|(entry, name)| SessionEvent::ItemTemplate {
            entry,
            info: Some(Box::new(crate::items::test_template(name))),
        })
        .collect(),
        2,
    );
    let pair = |field: u16, guid: u64| [(field, guid as u32), (field + 1, (guid >> 32) as u32)];
    let item = |guid: u64, entry: u32| {
        let mut pairs = vec![(FIELD_OBJECT_ENTRY, entry), (FIELD_ITEM_STACK_COUNT, 1)];
        pairs.extend(pair(FIELD_ITEM_OWNER, ME));
        pairs.extend(pair(FIELD_ITEM_CONTAINED, ME));
        SessionEvent::ItemCreate {
            guid,
            container: false,
            fields: ObjectFields::from_pairs(&pairs).into_created(ObjectType::Item),
        }
    };
    let main_hand = FIELD_PLAYER_INV_SLOT_HEAD + 2 * 15;
    rig.packets(
        vec![
            item(SWORD, SWORD_ENTRY),
            item(STONE, OLD),
            self_values(&pair(FIELD_PLAYER_PACK_SLOT_1, SWORD)),
            self_values(&pair(FIELD_PLAYER_PACK_SLOT_1 + 2, STONE)),
        ],
        2,
    );
    rig.app
        .world_mut()
        .resource_mut::<PendingItemOps>()
        .lock(SWORD);
    let events = &[
        "ITEM_LOCK_CHANGED",
        "ITEM_UNLOCKED",
        "PLAYER_EQUIPMENT_CHANGED",
        "BAG_UPDATE",
    ];
    let mut words = pair(FIELD_PLAYER_INV_SLOT_HEAD + 2 * 15, SWORD).to_vec();
    words.extend(pair(FIELD_PLAYER_PACK_SLOT_1, 0));
    let seen = rig.packets(vec![self_values(&words)], 2);
    assert_eq!(
        pin(&seen, events, &[]),
        [
            "ITEM_LOCK_CHANGED 16",
            "ITEM_UNLOCKED 16",
            "PLAYER_EQUIPMENT_CHANGED 16 1",
            "BAG_UPDATE 0",
            "BAG_UPDATE -2",
            "BAG_UPDATE -4",
        ],
        "the worn slot's watch runs first, in field order, and unlocks the item it now holds"
    );
    assert!(
        !rig.app
            .world()
            .resource::<PendingItemOps>()
            .is_locked(SWORD),
        "the equipped item is unlocked"
    );
    let seen = rig.packets(vec![self_values(&pair(main_hand, 0))], 2);
    assert_eq!(pin(&seen, events, &[]), ["PLAYER_EQUIPMENT_CHANGED 16"]);

    // An entry change to a cached record: the callback at once, then the queued quest update.
    let entry_events = &["BAG_UPDATE", "PLAYERBANKSLOTS_CHANGED", "QUEST_LOG_UPDATE"];
    let seen = rig.packets(
        vec![SessionEvent::ObjectValues {
            guid: STONE,
            fields: ObjectFields::from_pairs(&[(FIELD_OBJECT_ENTRY, CACHED)]),
        }],
        2,
    );
    assert_eq!(
        pin(&seen, entry_events, &[]),
        [
            "BAG_UPDATE 0",
            "BAG_UPDATE -2",
            "BAG_UPDATE -4",
            "QUEST_LOG_UPDATE"
        ]
    );
    let items = rig.app.world().resource::<crate::items::Items>();
    assert!(
        items.template_cached(OLD).is_none(),
        "the old record is dropped"
    );
    assert!(items.template_cached(CACHED).is_some());
    // An entry change to an uncached record: the quest update now, the callback when it lands.
    let seen = rig.packets(
        vec![SessionEvent::ObjectValues {
            guid: STONE,
            fields: ObjectFields::from_pairs(&[(FIELD_OBJECT_ENTRY, ASKED)]),
        }],
        2,
    );
    assert_eq!(pin(&seen, entry_events, &[]), ["QUEST_LOG_UPDATE"]);
    let seen = rig.packets(
        vec![SessionEvent::ItemTemplate {
            entry: ASKED,
            info: Some(Box::new(crate::items::test_template("Red Woolen Bag"))),
        }],
        2,
    );
    assert_eq!(
        pin(&seen, &["BAG_UPDATE"], &[]),
        ["BAG_UPDATE 0", "BAG_UPDATE -2", "BAG_UPDATE -4"],
        "the record callback runs when the record lands"
    );
}

/// A watched bag's slot watch runs once per slot field (`0x5d9600`, registered per slot at `0x5d9af8`): a swap inside a bag unlocks each slot's new item before that slot's `BAG_UPDATE`; the count watch unlocks every stack of a stackable kind at its place (`0x7077c1` -> `0x754d90`); the enchant watch unlocks nothing and returns on a charter (`0x707f6b`); `BAG_OPEN` names the container (`0x5d7110`).
#[test]
fn a_bag_slot_count_and_enchant_watch_each_run_their_own_handler() {
    use benilla_protocol::field::{
        FIELD_CONTAINER_NUM_SLOTS, FIELD_CONTAINER_SLOT_1, FIELD_ITEM_CONTAINED,
        FIELD_ITEM_ENCHANTMENT_1_1, FIELD_ITEM_OWNER, FIELD_ITEM_STACK_COUNT, FIELD_OBJECT_ENTRY,
        FIELD_PLAYER_INV_SLOT_HEAD, FIELD_PLAYER_PACK_SLOT_1,
    };
    const SACK: u64 = 0x4000_0000_0000_0500;
    const CLOTH: u64 = 0x4000_0000_0000_0501;
    const HIDE: u64 = 0x4000_0000_0000_0502;
    const ARROW_A: u64 = 0x4000_0000_0000_0503;
    const ARROW_B: u64 = 0x4000_0000_0000_0504;
    const CHARTER: u64 = 0x4000_0000_0000_0505;
    const ARROW_ENTRY: u32 = 2512;
    const CHARTER_ENTRY: u32 = 5863;
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    let mut arrows = crate::items::test_template("Rough Arrow");
    arrows.stackable = 200;
    let mut charter = crate::items::test_template("Guild Charter");
    charter.flags = 0x2000;
    rig.packets(
        vec![
            SessionEvent::ItemTemplate {
                entry: ARROW_ENTRY,
                info: Some(Box::new(arrows)),
            },
            SessionEvent::ItemTemplate {
                entry: CHARTER_ENTRY,
                info: Some(Box::new(charter)),
            },
        ],
        2,
    );
    let guid_pair =
        |field: u16, guid: u64| [(field, guid as u32), (field + 1, (guid >> 32) as u32)];
    let item = |guid: u64, entry: u32, inside: u64, extra: &[(u16, u32)]| {
        let mut pairs = vec![(FIELD_OBJECT_ENTRY, entry), (FIELD_ITEM_STACK_COUNT, 20)];
        pairs.extend(guid_pair(FIELD_ITEM_OWNER, ME));
        pairs.extend(guid_pair(FIELD_ITEM_CONTAINED, inside));
        pairs.extend_from_slice(extra);
        let bag = extra.iter().any(|&(f, _)| f == FIELD_CONTAINER_NUM_SLOTS);
        SessionEvent::ItemCreate {
            guid,
            container: bag,
            fields: ObjectFields::from_pairs(&pairs).into_created(if bag {
                ObjectType::Container
            } else {
                ObjectType::Item
            }),
        }
    };
    let lock_events = &[
        "BAG_UPDATE",
        "BAG_OPEN",
        "ITEM_LOCK_CHANGED",
        "ITEM_LOCKED",
        "ITEM_UNLOCKED",
        "PLAYERBANKSLOTS_CHANGED",
    ];
    let mut sack = vec![(FIELD_CONTAINER_NUM_SLOTS, 8)];
    sack.extend(guid_pair(FIELD_CONTAINER_SLOT_1, CLOTH));
    sack.extend(guid_pair(FIELD_CONTAINER_SLOT_1 + 2, HIDE));
    let mut pack = guid_pair(FIELD_PLAYER_INV_SLOT_HEAD + 2 * 19, SACK).to_vec();
    pack.extend(guid_pair(FIELD_PLAYER_PACK_SLOT_1 + 2, ARROW_A));
    pack.extend(guid_pair(FIELD_PLAYER_PACK_SLOT_1 + 4, CHARTER));
    pack.extend(guid_pair(FIELD_PLAYER_PACK_SLOT_1 + 8, ARROW_B));
    rig.packets(
        vec![
            item(SACK, 4496, ME, &sack),
            item(CLOTH, 2589, SACK, &[]),
            item(HIDE, 2318, SACK, &[]),
            item(ARROW_A, ARROW_ENTRY, ME, &[]),
            item(ARROW_B, ARROW_ENTRY, ME, &[]),
            item(CHARTER, CHARTER_ENTRY, ME, &[]),
            self_values(&pack),
        ],
        2,
    );

    // A swap inside the worn bag locks both items; the bag's two slot fields then run its slot watch twice, each unlocking its own new item.
    super::live::with_live_world(rig.app.world_mut(), |s| {
        s.run("PickupContainerItem(1, 1) PickupContainerItem(1, 2)")
    })
    .expect("a VM")
    .expect("the swap runs");
    let seen = rig.packets(Vec::new(), 2);
    let locks = pin(&seen, lock_events, &["lock11", "lock12"]);
    assert_eq!(
        locks[locks.len().saturating_sub(4)..],
        [
            "ITEM_LOCK_CHANGED 1 1 | lock11=1 lock12=nil",
            "ITEM_LOCKED 1 1 | lock11=1 lock12=nil",
            "ITEM_LOCK_CHANGED 1 2 | lock11=1 lock12=1",
            "ITEM_LOCKED 1 2 | lock11=1 lock12=1",
        ]
    );
    let mut swap = guid_pair(FIELD_CONTAINER_SLOT_1, HIDE).to_vec();
    swap.extend(guid_pair(FIELD_CONTAINER_SLOT_1 + 2, CLOTH));
    let seen = rig.packets(
        vec![SessionEvent::ObjectValues {
            guid: SACK,
            fields: ObjectFields::from_pairs(&swap),
        }],
        2,
    );
    assert_eq!(
        pin(&seen, lock_events, &["lock11", "lock12"]),
        [
            "ITEM_LOCK_CHANGED 1 1 | lock11=nil lock12=1",
            "ITEM_UNLOCKED 1 1 | lock11=nil lock12=1",
            "BAG_UPDATE 1 | lock11=nil lock12=1",
            "ITEM_LOCK_CHANGED 1 2 | lock11=nil lock12=nil",
            "ITEM_UNLOCKED 1 2 | lock11=nil lock12=nil",
            "BAG_UPDATE 1 | lock11=nil lock12=nil",
        ],
        "one handler run per slot field, each unlock before its slot's update"
    );

    // A merge locks both arrow stacks; the count watch on one unlocks every stack of the kind in the search's order, each read at its own event.
    super::live::with_live_world(rig.app.world_mut(), |s| {
        s.run("PickupContainerItem(0, 2) PickupContainerItem(0, 5)")
    })
    .expect("a VM")
    .expect("the merge runs");
    rig.packets(Vec::new(), 2);
    let seen = rig.packets(
        vec![SessionEvent::ObjectValues {
            guid: ARROW_B,
            fields: ObjectFields::from_pairs(&[(FIELD_ITEM_STACK_COUNT, 40)]),
        }],
        2,
    );
    assert_eq!(
        pin(&seen, lock_events, &["lock02", "lock05"]),
        [
            "ITEM_LOCK_CHANGED 0 2 | lock02=nil lock05=1",
            "ITEM_UNLOCKED 0 2 | lock02=nil lock05=1",
            "ITEM_LOCK_CHANGED 0 5 | lock02=nil lock05=nil",
            "ITEM_UNLOCKED 0 5 | lock02=nil lock05=nil",
            "BAG_UPDATE 0 | lock02=nil lock05=nil",
            "BAG_UPDATE -2 | lock02=nil lock05=nil",
            "BAG_UPDATE -4 | lock02=nil lock05=nil",
        ]
    );

    // A locked stack's enchant change unlocks nothing and updates its bag; a charter's fires nothing at all.
    super::live::with_live_world(rig.app.world_mut(), |s| {
        s.run("PickupContainerItem(0, 5) PickupContainerItem(0, 7)")
    })
    .expect("a VM")
    .expect("the move runs");
    rig.packets(Vec::new(), 2);
    let seen = rig.packets(
        vec![
            SessionEvent::ObjectValues {
                guid: ARROW_B,
                fields: ObjectFields::from_pairs(&[(FIELD_ITEM_ENCHANTMENT_1_1, 1)]),
            },
            SessionEvent::ObjectValues {
                guid: CHARTER,
                fields: ObjectFields::from_pairs(&[(FIELD_ITEM_ENCHANTMENT_1_1, 1)]),
            },
        ],
        2,
    );
    assert_eq!(
        pin(&seen, lock_events, &["lock05"]),
        [
            "BAG_UPDATE 0 | lock05=1",
            "BAG_UPDATE -2 | lock05=1",
            "BAG_UPDATE -4 | lock05=1",
        ],
        "the stack stays locked; the charter's watch returns before any update"
    );

    // `SMSG_OPEN_CONTAINER`: our own guid is the backpack, the worn bag its id, anything else nothing.
    let seen = rig.packets(
        vec![
            SessionEvent::OpenContainer { item: SACK },
            SessionEvent::OpenContainer { item: ME },
            SessionEvent::OpenContainer { item: CHARTER },
        ],
        2,
    );
    assert_eq!(
        pin(&seen, &["BAG_OPEN"], &["slots1"]),
        ["BAG_OPEN 1 | slots1=8", "BAG_OPEN 0 | slots1=8"]
    );
}

/// A Lua bag index reads the watched-bag array (`0x5d6f20`): a bank bag answers only with a banker set (`0x5d6f34`-`0x5d6f3f`), and during a bank-bag slot's update the inventory watch's `PLAYERBANKSLOTS_CHANGED` (`0x6deed6`) still reads the old bag, as does the refresh's `BAG_CLOSED` (`0x5d9a87`), until the array takes the new one (`0x5d9a96`).
#[test]
fn a_bag_index_reads_the_watched_bag_behind_the_banker_gate() {
    use benilla_protocol::field::{
        FIELD_CONTAINER_NUM_SLOTS, FIELD_ITEM_CONTAINED, FIELD_ITEM_OWNER, FIELD_OBJECT_ENTRY,
        FIELD_PLAYER_BANK_BAG_SLOT_1,
    };
    const POUCH: u64 = 0x4000_0000_0000_0601;
    const SACK: u64 = 0x4000_0000_0000_0602;
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    let guid_pair =
        |field: u16, guid: u64| [(field, guid as u32), (field + 1, (guid >> 32) as u32)];
    let bag = |guid: u64, entry: u32, slots: u32| {
        let mut pairs = vec![
            (FIELD_OBJECT_ENTRY, entry),
            (FIELD_CONTAINER_NUM_SLOTS, slots),
        ];
        pairs.extend(guid_pair(FIELD_ITEM_OWNER, ME));
        pairs.extend(guid_pair(FIELD_ITEM_CONTAINED, ME));
        SessionEvent::ItemCreate {
            guid,
            container: true,
            fields: ObjectFields::from_pairs(&pairs).into_created(ObjectType::Container),
        }
    };
    let events = &[
        "PLAYERBANKSLOTS_CHANGED",
        "BAG_CLOSED",
        "BAG_UPDATE",
        "BANKFRAME_OPENED",
    ];
    let verbs = &["slots5"];
    let seen = rig.packets(
        vec![
            bag(POUCH, 828, 6),
            bag(SACK, 4496, 8),
            self_values(&guid_pair(FIELD_PLAYER_BANK_BAG_SLOT_1, POUCH)),
        ],
        2,
    );
    assert_eq!(
        pin(&seen, events, verbs),
        [
            "PLAYERBANKSLOTS_CHANGED 29 | slots5=0",
            "BAG_UPDATE 0 | slots5=0",
            "BAG_UPDATE -2 | slots5=0",
            "BAG_UPDATE -4 | slots5=0",
            "BAG_UPDATE 5 | slots5=0",
        ],
        "no banker: the bank bag answers nothing"
    );
    let seen = rig.packets(
        vec![
            banker_create(0x20000),
            SessionEvent::ShowBank { banker: BANKER },
        ],
        2,
    );
    assert_eq!(
        pin(&seen, events, &["slots5", "name5"]),
        ["BANKFRAME_OPENED | slots5=6 name5=nil"],
        "a banker set: the watched pouch answers; its record is not cached"
    );
    let seen = rig.packets(
        vec![self_values(&guid_pair(FIELD_PLAYER_BANK_BAG_SLOT_1, SACK))],
        2,
    );
    assert_eq!(
        pin(&seen, events, verbs),
        [
            "PLAYERBANKSLOTS_CHANGED 29 | slots5=6",
            "BAG_UPDATE 0 | slots5=6",
            "BAG_UPDATE -2 | slots5=6",
            "BAG_UPDATE -4 | slots5=6",
            "BAG_CLOSED 5 | slots5=6",
            "BAG_UPDATE 5 | slots5=8",
        ],
        "the old bag answers until the refresh takes the new one"
    );
}

/// A refundable purchase's record is asked by the item's post-init, by the merchant list for one held before the player, and answered into the verbs at the call; the result's lines print from its handler (`0x70ae34`, `0x6d1760`, `0x6d1650`, `0x6d9b40`).
#[test]
fn a_refund_record_is_asked_read_at_the_call_and_its_result_prints_in_the_handler() {
    use crate::net::ClientCommand;
    use benilla_protocol::field::{
        FIELD_ITEM_CONTAINED, FIELD_ITEM_OWNER, FIELD_ITEM_STACK_COUNT, FIELD_OBJECT_ENTRY,
        FIELD_PLAYER_PACK_SLOT_1,
    };
    use benilla_protocol::messages::{PurchaseCost, RefundRecord};
    const BADGE_BOUGHT: u32 = 40_001;
    const EARLY: u64 = 0x4000_0000_0000_0500;
    const LATER: u64 = 0x4000_0000_0000_0501;
    let item = |guid: u64| SessionEvent::ItemCreate {
        guid,
        container: false,
        fields: ObjectFields::from_pairs(&[
            (FIELD_OBJECT_ENTRY, BADGE_BOUGHT),
            (FIELD_ITEM_OWNER, ME as u32),
            (FIELD_ITEM_OWNER + 1, (ME >> 32) as u32),
            (FIELD_ITEM_CONTAINED, ME as u32),
            (FIELD_ITEM_CONTAINED + 1, (ME >> 32) as u32),
            (FIELD_ITEM_STACK_COUNT, 1),
        ])
        .into_created(ObjectType::Item),
    };
    let mut template = crate::items::test_template("Charz Purchase");
    template.flags = 0x1000;
    let template = SessionEvent::ItemTemplate {
        entry: BADGE_BOUGHT,
        info: Some(Box::new(template)),
    };
    let slot = |field: u16, guid: u64| [(field, guid as u32), (field + 1, (guid >> 32) as u32)];
    // An item created ahead of our own create: its post-init has no player to ask for.
    let Some(mut rig) = Rig::login_around(
        vec![template, item(EARLY)],
        &[
            OBJECT_TYPE_PLAYER,
            slot(FIELD_PLAYER_PACK_SLOT_1, EARLY)[0],
            slot(FIELD_PLAYER_PACK_SLOT_1, EARLY)[1],
        ],
        Vec::new(),
    ) else {
        return;
    };
    let (tx, rx) = crossbeam_channel::unbounded();
    rig.app.insert_resource(crate::net::NetCommands(tx));
    let asks = |rx: &crossbeam_channel::Receiver<ClientCommand>| -> Vec<u64> {
        rx.try_iter()
            .filter_map(|c| match c {
                ClientCommand::ItemRefundInfo { item } => Some(item),
                _ => None,
            })
            .collect()
    };
    assert_eq!(rig.now().verbs["refund01"], "nil");

    // The merchant list asks the held purchase without a record, once.
    let list = || SessionEvent::VendorInventory {
        vendor: 0xF130_0000_0000_0042,
        items: Vec::new(),
        error: None,
    };
    rig.packets(vec![list()], 1);
    assert_eq!(
        asks(&rx),
        [EARLY],
        "the list asks the purchase held before the player"
    );
    rig.packets(vec![list()], 1);
    assert!(
        asks(&rx).is_empty(),
        "the pending bit holds the second list"
    );

    // The record reads at the call, the frame its answer lands.
    let record = RefundRecord {
        cost: PurchaseCost {
            honor: 2000,
            ..Default::default()
        },
        played_at_purchase: 100,
    };
    rig.send(SessionEvent::ItemRefundInfo {
        item: EARLY,
        record,
    });
    rig.app.update();
    assert_eq!(rig.live_now().verbs["refund01"], "2000");

    // An item made with the player held asks from its post-init.
    rig.packets(vec![item(LATER)], 1);
    assert_eq!(asks(&rx), [LATER], "the post-init asks the new purchase");

    // The result prints its lines in its handler, ahead of the next packet's events.
    let seen = rig.packets(
        vec![
            SessionEvent::ItemRefundResult {
                item: EARLY,
                result: 0,
                cost: Some(record.cost),
            },
            SessionEvent::ObjectDestroyed(EARLY),
        ],
        2,
    );
    let lines: Vec<&str> = seen
        .iter()
        .filter(|s| s.event == "CHAT_MSG_SYSTEM")
        .map(|s| s.args.as_str())
        .collect();
    assert_eq!(
        lines.len(),
        2,
        "the refund line and the honor line: {lines:?}"
    );
    assert!(lines[1].contains("2000"), "the honor refunded: {lines:?}");
    assert_eq!(
        rig.live_now().verbs["refund01"],
        "nil",
        "the record went with its item"
    );
}

/// The bank's open takes the interact target from the vendor holding it (`0x57b909` -> `0x51fce0`): the vendor closes.
#[test]
fn the_banks_open_closes_the_vendor_holding_the_target() {
    const VENDOR: u64 = 0xF130_0000_0200_0002;
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    rig.packets(vec![banker_create(0x20000), npc_create(VENDOR, 0x80)], 1);
    let row = benilla_protocol::messages::VendorItem {
        slot: 1,
        entry: 159,
        display_id: 18084,
        current_count: 0xFFFF_FFFF,
        price: 5,
        max_durability: 0,
        buy_count: 1,
        extended_cost: 0,
    };
    rig.packets(
        vec![SessionEvent::VendorInventory {
            vendor: VENDOR,
            items: vec![row],
            error: None,
        }],
        2,
    );
    let open = |rig: &App| {
        use crate::ui_session::NpcSession;
        rig.world()
            .resource::<crate::ui_merchant::MerchantOpen>()
            .npc()
    };
    assert_eq!(open(&rig.app), Some(VENDOR), "the vendor is open");
    let seen = rig.packets(vec![SessionEvent::ShowBank { banker: BANKER }], 3);
    assert_eq!(open(&rig.app), None, "the bank's open closed the vendor");
    let order = fired(&seen, &["MERCHANT_CLOSED", "BANKFRAME_OPENED"]);
    assert!(
        order.iter().any(|e| e == "MERCHANT_CLOSED"),
        "the vendor's close fired: {order:?}"
    );
}

/// The guild bank's handlers fire in 12340's turn with the verbs reading the bank at the call: the hello's list fires each tab row's `GUILDBANK_UPDATE_TABS` after that row and before the slots (`0x5a73f6`), then `GUILDBANKFRAME_OPENED` (`0x5a774c`); an open bank's list waits on an uncached record and fires `GUILDBANKBAGSLOTS_CHANGED` from its callback (`0x5a767c`, `0x5a47a0`), or at once with the record cached (`0x5a76c7`); the log waits on its names (`0x5a4a34`, `0x5a47d0`); the money, the withdrawal and the text fire from their handlers (`0x6d97ad`, `0x5a4ab0`, `0x5a4ae0`); the quit closes the bank (`0x6cc630`).
#[test]
fn the_guild_bank_fires_from_its_handlers_with_the_bank_read_at_the_call() {
    use benilla_protocol::messages::{
        guild_command, guild_command_error, guild_event, GuildBankList, GuildBankLog,
        GuildBankLogEntry, GuildBankSlot, GuildBankTab, GuildCommandResult, GuildEventNotice,
    };
    const BANK: &[&str] = &[
        "GUILDBANK_UPDATE_TABS",
        "GUILDBANKFRAME_OPENED",
        "GUILDBANKFRAME_CLOSED",
        "GUILDBANKBAGSLOTS_CHANGED",
        "GUILDBANKLOG_UPDATE",
        "GUILDBANK_UPDATE_MONEY",
        "GUILDBANK_UPDATE_WITHDRAWMONEY",
        "GUILDBANK_UPDATE_TEXT",
    ];
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    let (tx, _rx) = crossbeam_channel::unbounded();
    rig.app.insert_resource(crate::net::NetCommands(tx));
    // A guild banker held beside us, so the walk-away watchdog keeps it open.
    const VAULT: u64 = BANKER + 9;
    rig.packets(vec![npc_create(VAULT, 0x80_0000)], 2);
    {
        let world = rig.app.world_mut();
        let net = crate::net::NetCommands(world.resource::<crate::net::NetCommands>().0.clone());
        world
            .non_send_resource_mut::<crate::ui_guild_bank::GuildBankState>()
            .talk(VAULT, &net);
    }
    let slot = |slot: u8, item_id: u32, count: u32| GuildBankSlot {
        slot,
        item_id,
        count,
        ..Default::default()
    };
    let template = |entry: u32| SessionEvent::ItemTemplate {
        entry,
        info: Some(Box::new(crate::items::test_template("Linen Cloth"))),
    };
    let list = |slots: Vec<GuildBankSlot>, tabs: Option<Vec<GuildBankTab>>| {
        SessionEvent::GuildBankList(GuildBankList {
            money: 1234,
            tab: 0,
            withdrawals_remaining: 5,
            tabs,
            slots,
        })
    };
    let tab = |name: &str| GuildBankTab {
        name: name.into(),
        icon: "INV_Misc_Bag_10".into(),
    };
    // The hello's answer: each row's event after that row, the slots after both, then the open.
    let seen = rig.packets(
        vec![
            template(2589),
            list(vec![slot(3, 2589, 20)], Some(vec![tab("Mats"), tab("Ore")])),
        ],
        2,
    );
    assert_eq!(
        pin(
            &seen,
            BANK,
            &["gbtabs", "gbtab1", "gbtab2", "gbleft", "gbcount4", "gbmoney"]
        ),
        [
            "GUILDBANK_UPDATE_TABS | gbtabs=2 gbtab1=Mats gbtab2=Tab2 gbleft=5 gbcount4=0 gbmoney=1234",
            "GUILDBANK_UPDATE_TABS | gbtabs=2 gbtab1=Mats gbtab2=Ore gbleft=5 gbcount4=0 gbmoney=1234",
            "GUILDBANKFRAME_OPENED | gbtabs=2 gbtab1=Mats gbtab2=Ore gbleft=5 gbcount4=20 gbmoney=1234",
        ]
    );
    // An open bank's list with a record not cached waits for it; its landing fires the event.
    let seen = rig.packets(vec![list(vec![slot(3, 4306, 7)], None)], 2);
    assert!(fired(&seen, BANK).is_empty(), "{:?}", fired(&seen, BANK));
    let seen = rig.packets(vec![template(4306)], 2);
    assert_eq!(
        pin(&seen, BANK, &["gbcount4"]),
        ["GUILDBANKBAGSLOTS_CHANGED | gbcount4=7"]
    );
    // The record cached first, the list fires at once.
    let seen = rig.packets(vec![list(vec![slot(3, 4306, 9)], None)], 2);
    assert_eq!(
        pin(&seen, BANK, &["gbcount4"]),
        ["GUILDBANKBAGSLOTS_CHANGED | gbcount4=9"]
    );
    // The log waits on its row's name; the name's answer fires it.
    const WRITER: u64 = 0x42;
    let log = SessionEvent::GuildBankLog(GuildBankLog {
        tab: 0,
        entries: vec![GuildBankLogEntry {
            kind: 1,
            player: WRITER,
            item_id: 4306,
            count: 2,
            ..Default::default()
        }],
    });
    let seen = rig.packets(vec![log], 2);
    assert!(fired(&seen, BANK).is_empty(), "{:?}", fired(&seen, BANK));
    let name = SessionEvent::PlayerName {
        guid: WRITER,
        name: "Benilla".into(),
        realm: String::new(),
        race: 1,
        gender: 0,
        class: 1,
        declensions: None,
    };
    let seen = rig.packets(vec![name], 2);
    assert_eq!(
        pin(&seen, BANK, &["gblog", "gblogname"]),
        ["GUILDBANKLOG_UPDATE | gblog=1 gblogname=Benilla"]
    );
    // The withdrawal, the money and the text, each from its handler in wire order.
    let money = SessionEvent::GuildEvent(GuildEventNotice {
        event: guild_event::BANK_MONEY_SET,
        params: vec!["0000000000002710".into()],
        guid: None,
    });
    let seen = rig.packets(
        vec![
            SessionEvent::GuildBankWithdrawMoney { remaining: 500 },
            money,
            SessionEvent::GuildBankText {
                tab: 0,
                text: "hello".into(),
            },
        ],
        2,
    );
    assert_eq!(
        pin(&seen, BANK, &["gbmoney", "gbwithdraw", "gbtext"]),
        [
            "GUILDBANK_UPDATE_WITHDRAWMONEY | gbmoney=1234 gbwithdraw=500 gbtext=",
            "GUILDBANK_UPDATE_MONEY | gbmoney=10000 gbwithdraw=500 gbtext=",
            "GUILDBANK_UPDATE_TEXT 1 | gbmoney=10000 gbwithdraw=500 gbtext=hello",
        ]
    );
    // Leaving the guild closes the bank, and the stock window's hide calls `CloseGuildBankFrame`, which fires it again (`0x5a4500`); a list then has no banker and none asked: closed once more (`0x5a76ef`).
    let quit = SessionEvent::GuildCommandResult(GuildCommandResult {
        command: guild_command::QUIT,
        name: "Benilla".into(),
        result: guild_command_error::PLAYER_NO_MORE_IN_GUILD,
    });
    let seen = rig.packets(vec![quit, list(Vec::new(), None)], 2);
    assert_eq!(
        fired(&seen, BANK),
        [
            "GUILDBANKFRAME_CLOSED",
            "GUILDBANKFRAME_CLOSED",
            "GUILDBANKFRAME_CLOSED"
        ]
    );
}

/// One record's callbacks run in ask order across subsystems, as 12340 links every ask to the tail of the record's one list (`0x67ca82`, `0x67cab1`): our bag item's post-init (`0x70af17`) and an open guild bank's list (`0x5a7664`) wait on the same entry, and its answer runs them in the order they asked, the post-init's `BAG_UPDATE`s (`0x70ae0b`) and `GUILDBANKBAGSLOTS_CHANGED` (`0x5a47a0`) either way round; a verb's callback, the currency build's (`0x5b01d7`), joins the same list.
#[test]
fn one_records_callbacks_run_in_ask_order_across_subsystems() {
    use benilla_protocol::field::{
        FIELD_ITEM_CONTAINED, FIELD_ITEM_OWNER, FIELD_ITEM_STACK_COUNT, FIELD_OBJECT_ENTRY,
        FIELD_PLAYER_KNOWN_CURRENCIES, FIELD_PLAYER_PACK_SLOT_1,
    };
    use benilla_protocol::messages::{GuildBankList, GuildBankSlot, GuildBankTab};
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    let (tx, _rx) = crossbeam_channel::unbounded();
    rig.app.insert_resource(crate::net::NetCommands(tx));
    const VAULT: u64 = BANKER + 9;
    rig.packets(vec![npc_create(VAULT, 0x80_0000)], 2);
    {
        let world = rig.app.world_mut();
        let net = crate::net::NetCommands(world.resource::<crate::net::NetCommands>().0.clone());
        world
            .non_send_resource_mut::<crate::ui_guild_bank::GuildBankState>()
            .talk(VAULT, &net);
    }
    let list = |entry: u32, tabs: Option<Vec<GuildBankTab>>| {
        SessionEvent::GuildBankList(GuildBankList {
            money: 0,
            tab: 0,
            withdrawals_remaining: 5,
            tabs,
            slots: vec![GuildBankSlot {
                slot: 3,
                item_id: entry,
                count: 1,
                ..Default::default()
            }],
        })
    };
    let template = |entry: u32| SessionEvent::ItemTemplate {
        entry,
        info: Some(Box::new(crate::items::test_template("Linen Cloth"))),
    };
    // Our backpack's slot `n` takes an item of `entry` whose record is not cached.
    let bag_item = |guid: u64, entry: u32, n: u16| {
        let slot = FIELD_PLAYER_PACK_SLOT_1 + 2 * n;
        vec![
            SessionEvent::ItemCreate {
                guid,
                container: false,
                fields: ObjectFields::from_pairs(&[
                    (FIELD_OBJECT_ENTRY, entry),
                    (FIELD_ITEM_OWNER, ME as u32),
                    (FIELD_ITEM_OWNER + 1, (ME >> 32) as u32),
                    (FIELD_ITEM_CONTAINED, ME as u32),
                    (FIELD_ITEM_CONTAINED + 1, (ME >> 32) as u32),
                    (FIELD_ITEM_STACK_COUNT, 1),
                ])
                .into_created(ObjectType::Item),
            },
            self_values(&[(slot, guid as u32), (slot + 1, (guid >> 32) as u32)]),
        ]
    };
    let tab = GuildBankTab {
        name: "Mats".into(),
        icon: "INV_Misc_Bag_10".into(),
    };
    rig.packets(vec![template(2589), list(2589, Some(vec![tab]))], 2);
    const EVENTS: &[&str] = &["BAG_UPDATE", "GUILDBANKBAGSLOTS_CHANGED"];
    // The bag item asks first, then the bank's list.
    rig.packets(bag_item(0x4000_0000_0000_0401, 4306, 3), 2);
    let seen = rig.packets(vec![list(4306, None)], 2);
    assert!(fired(&seen, EVENTS).iter().all(|e| e == "BAG_UPDATE"));
    let seen = rig.packets(vec![template(4306)], 2);
    assert_eq!(
        fired(&seen, EVENTS),
        [
            "BAG_UPDATE",
            "BAG_UPDATE",
            "BAG_UPDATE",
            "GUILDBANKBAGSLOTS_CHANGED"
        ],
        "the post-init asked first"
    );
    // The bank's list asks first, then the bag item.
    rig.packets(vec![list(2592, None)], 2);
    rig.packets(bag_item(0x4000_0000_0000_0402, 2592, 4), 2);
    let seen = rig.packets(vec![template(2592)], 2);
    assert_eq!(
        fired(&seen, EVENTS),
        [
            "GUILDBANKBAGSLOTS_CHANGED",
            "BAG_UPDATE",
            "BAG_UPDATE",
            "BAG_UPDATE"
        ],
        "the bank's list asked first"
    );
    // A verb's callback joins the same list: the bag item asks, then the currency build inside the known-currencies watch (`0x5b01d7`).
    const BADGE: u32 = 29434;
    let bit = rig
        .app
        .world()
        .resource::<crate::ui_currency::Currencies>()
        .0
        .types
        .iter()
        .find(|t| t.item_id == BADGE)
        .map(|t| t.bit_index)
        .expect("the badge's row");
    let known = 1u64 << (bit - 1);
    rig.packets(bag_item(0x4000_0000_0000_0403, BADGE, 5), 2);
    rig.packets(
        vec![self_values(&[
            (FIELD_PLAYER_KNOWN_CURRENCIES, known as u32),
            (FIELD_PLAYER_KNOWN_CURRENCIES + 1, (known >> 32) as u32),
        ])],
        2,
    );
    let seen = rig.packets(vec![template(BADGE)], 2);
    assert_eq!(
        fired(&seen, &["BAG_UPDATE", "CURRENCY_DISPLAY_UPDATE"]),
        [
            "BAG_UPDATE",
            "BAG_UPDATE",
            "BAG_UPDATE",
            "CURRENCY_DISPLAY_UPDATE"
        ],
        "the post-init asked before the build"
    );
}

/// The walk-away watchdog closes a bank whose banker is gone through the interaction close, its events fired as it runs, ahead of the frame's input: `BANKFRAME_CLOSED` with the banker still set (`0x512fa7` -> `0x57b8e6`), and the guild bank's leg through the banker setter (`0x512fd1` -> `0x5a4270`).
#[test]
fn the_walk_away_watchdog_closes_both_banks_with_their_events() {
    use benilla_protocol::messages::{GuildBankList, GuildBankTab};
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    let (tx, _rx) = crossbeam_channel::unbounded();
    rig.app.insert_resource(crate::net::NetCommands(tx));
    rig.packets(
        vec![
            banker_create(0x20000),
            SessionEvent::ShowBank { banker: BANKER },
        ],
        2,
    );
    // A plain destroy runs no death: the watchdog finds the banker gone.
    let seen = rig.packets(vec![SessionEvent::ObjectDestroyed(BANKER)], 2);
    assert_eq!(
        pin(&seen, &["BANKFRAME_CLOSED"], &["bankopen"]),
        [
            "BANKFRAME_CLOSED | bankopen=true",
            "BANKFRAME_CLOSED | bankopen=false",
        ]
    );
    const VAULT: u64 = BANKER + 9;
    rig.packets(vec![npc_create(VAULT, 0x80_0000)], 2);
    {
        let world = rig.app.world_mut();
        let net = crate::net::NetCommands(world.resource::<crate::net::NetCommands>().0.clone());
        world
            .non_send_resource_mut::<crate::ui_guild_bank::GuildBankState>()
            .talk(VAULT, &net);
    }
    let list = SessionEvent::GuildBankList(GuildBankList {
        tabs: Some(vec![GuildBankTab {
            name: "Mats".into(),
            icon: String::new(),
        }]),
        ..Default::default()
    });
    let seen = rig.packets(vec![list], 2);
    assert_eq!(fired(&seen, &["GUILDBANKFRAME_OPENED"]).len(), 1);
    let seen = rig.packets(vec![SessionEvent::ObjectDestroyed(VAULT)], 2);
    assert_eq!(
        pin(&seen, &["GUILDBANKFRAME_CLOSED"], &["gbtabs"]),
        [
            "GUILDBANKFRAME_CLOSED | gbtabs=1",
            "GUILDBANKFRAME_CLOSED | gbtabs=1",
        ]
    );
}

/// An item record is read from the item cache at each call (`0x67ca30`): a miss asks the cache once (`CMSG_ITEM_QUERY_SINGLE`) and answers nothing; the answer's handler stores it and runs the UI's callbacks on it, so a tooltip waiting on it repaints in the packet's frame, and the next `GetItemInfo` and `GetItemStats` read it with no feed between.
#[test]
fn an_item_record_reads_at_the_call_and_its_answer_repaints_from_the_handler() {
    use crate::net::ClientCommand;
    const CLOTH: u32 = 2589;
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    let (tx, rx) = crossbeam_channel::unbounded();
    rig.app.insert_resource(crate::net::NetCommands(tx));
    let eval = |rig: &mut Rig, chunk: &str| {
        super::live::with_live_world(rig.app.world_mut(), |s| s.eval::<String>(chunk))
            .expect("a VM")
            .expect("the chunk runs")
    };
    let info = r#"return tostring((GetItemInfo(2589)))"#;
    assert_eq!(eval(&mut rig, info), "nil", "a miss answers nothing");
    eval(
        &mut rig,
        r#"GameTooltip:SetOwner(UIParent, "ANCHOR_NONE") GameTooltip:SetHyperlink("item:2589") return tostring(GameTooltipTextLeft1:GetText())"#,
    );
    let asks: Vec<u32> = rx
        .try_iter()
        .filter_map(|c| match c {
            ClientCommand::ItemQuery { entry, .. } => Some(entry),
            _ => None,
        })
        .collect();
    assert_eq!(asks, [CLOTH], "the misses ask once");
    let mut cloth = crate::items::test_template("Linen Cloth");
    cloth.stats = vec![(7, 10)];
    rig.packets(
        vec![SessionEvent::ItemTemplate {
            entry: CLOTH,
            info: Some(Box::new(cloth)),
        }],
        1,
    );
    assert_eq!(
        eval(&mut rig, "return tostring(GameTooltipTextLeft1:GetText())"),
        "Linen Cloth",
        "the handler's callback repainted the waiting tooltip"
    );
    assert_eq!(eval(&mut rig, info), "Linen Cloth");
    assert_eq!(
        eval(
            &mut rig,
            r#"return tostring(GetItemStats("item:2589")["ITEM_MOD_STAMINA_SHORT"])"#
        ),
        "10",
        "the stat lines read the record at the call"
    );
}

/// Each window whose open now takes the interact target closes the vendor holding it inside that open, `MERCHANT_CLOSED` ahead of its own show: the quest frame (`0x58c6cc`), the auctioneer (`0x59f008`), the stable (`0x5a11d9`), the mailbox (`0x56dbe2`), the battlemaster (`0x54970f`) and the trade window (`0x5874cb` -> `0x587124`); the vendor's next open then closes that window in turn through its leg of the interaction close (`0x512e60`).
#[test]
fn every_window_open_takes_the_interact_target_from_the_vendor_holding_it() {
    use benilla_protocol::messages::{BattlefieldList, QuestGiverList, TradeStatus};
    const VENDOR: u64 = BANKER + 0x10;
    const NEW: u64 = BANKER + 0x11;
    const PARTNER: u64 = 0x0000_0000_0000_0777;
    // Both Human (template 1), so the partner passes the watchdog's cooperation check (`0x51feba`).
    let human = (benilla_protocol::field::FIELD_UNIT_FACTIONTEMPLATE, 1);
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER, human]) else {
        return;
    };
    let (tx, _rx) = crossbeam_channel::unbounded();
    rig.app.insert_resource(crate::net::NetCommands(tx));
    rig.packets(
        vec![
            npc_create(VENDOR, GOSSIP_NPC),
            npc_create(NEW, GOSSIP_NPC),
            player_create_with(PARTNER, &[human]),
        ],
        2,
    );
    let vendor = || SessionEvent::VendorInventory {
        vendor: VENDOR,
        items: Vec::new(),
        error: Some(0),
    };
    let target = |rig: &Rig| {
        rig.app
            .world()
            .resource::<crate::ui_session::InteractTarget>()
            .0
    };
    type Open = fn(&World) -> bool;
    let opens: [(&str, SessionEvent, &str, u64, Open); 6] = [
        (
            "quest",
            SessionEvent::QuestGreeting(QuestGiverList {
                npc: NEW,
                ..Default::default()
            }),
            "QUEST_GREETING",
            NEW,
            |w| {
                w.non_send_resource::<crate::ui_quest::QuestGiver>()
                    .is_open()
            },
        ),
        (
            "auction",
            SessionEvent::AuctionHello {
                auctioneer: NEW,
                house_id: 1,
                enabled: true,
            },
            "AUCTION_HOUSE_SHOW",
            NEW,
            |w| {
                w.resource::<crate::ui_auction::AuctionOpen>()
                    .auctioneer
                    .is_some()
            },
        ),
        (
            "stable",
            SessionEvent::ListStabledPets {
                npc: NEW,
                num_stable_slots: 0,
                pets: Vec::new(),
            },
            "PET_STABLE_SHOW",
            NEW,
            |w| {
                use crate::ui_session::NpcSession;
                w.resource::<crate::ui_stable::StableOpen>().npc().is_some()
            },
        ),
        (
            "mailbox",
            SessionEvent::ShowMailbox { mailbox: NEW },
            "MAIL_SHOW",
            NEW,
            |w| w.resource::<crate::ui_mail::MailOpen>().mailbox.is_some(),
        ),
        (
            "battlemaster",
            SessionEvent::BattlefieldList(BattlefieldList {
                battlemaster: NEW,
                bg_type: 2,
                ..Default::default()
            }),
            "BATTLEFIELDS_SHOW",
            NEW,
            |w| {
                w.resource::<crate::ui_battlefield::Battlefield>()
                    .battlemaster()
                    .is_some()
            },
        ),
        (
            "trade",
            SessionEvent::TradeStatus {
                status: TradeStatus::OpenWindow { trade_id: 1 },
            },
            "TRADE_SHOW",
            PARTNER,
            |w| w.resource::<crate::ui_trade::TradeSession>().is_open(),
        ),
    ];
    for (what, open, show, holder, is_open) in opens {
        rig.packets(vec![vendor()], 2);
        assert_eq!(target(&rig), Some(VENDOR), "{what}: the vendor holds it");
        if what == "trade" {
            rig.app
                .world_mut()
                .resource_mut::<crate::ui_trade::TradeSession>()
                .begin(PARTNER);
        }
        let seen = rig.packets(vec![open], 3);
        let order = fired(&seen, &["MERCHANT_CLOSED", show]);
        assert_eq!(
            order.first().map(String::as_str),
            Some("MERCHANT_CLOSED"),
            "{what}: the vendor closes inside the open, ahead of its show: {order:?}"
        );
        assert_eq!(
            fired(&seen, &["MERCHANT_CLOSED"]).len(),
            1,
            "{what}: one close, from its leg"
        );
        assert_eq!(target(&rig), Some(holder), "{what} holds the target");
        assert!(is_open(rig.app.world()), "{what} opened");
        // The vendor's next open takes it back through the window's own leg.
        rig.packets(vec![vendor()], 2);
        assert!(
            !is_open(rig.app.world()),
            "{what}: the vendor's open closed it"
        );
        assert_eq!(target(&rig), Some(VENDOR));
    }
}

/// The quest frame's set-current takes the interact target even from the giver holding it, so a creature's greeting then details at the same NPC fire `QUEST_FINISHED` between (`0x58c6cc` -> `0x51fd1c` -> `0x512f1a`); a GameObject giver already holding it skips the set, and fires none (`0x58c6a9`-`0x58c6c6`).
#[test]
fn a_creature_givers_next_panel_finishes_the_last_and_a_gameobjects_does_not() {
    use benilla_protocol::messages::{QuestDetails, QuestGiverList};
    const GIVER: u64 = BANKER + 0x20;
    const BOARD: u64 = 0xF110_0000_0300_0021;
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    let (tx, _rx) = crossbeam_channel::unbounded();
    rig.app.insert_resource(crate::net::NetCommands(tx));
    let board = SessionEvent::ObjectCreate {
        guid: BOARD,
        kind: EntityKind::GameObject,
        display_id: None,
        position: [-8949.0, -130.0, 83.5],
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
        // `OBJECT_FIELD_TYPE`: OBJECT | GAMEOBJECT.
        fields: ObjectFields::from_pairs(&[(2, 0x21)]).into_created(ObjectType::GameObject),
    };
    rig.packets(vec![npc_create(GIVER, GOSSIP_NPC), board], 2);
    for (giver, finishes) in [(GIVER, 1), (BOARD, 0)] {
        let greeting = SessionEvent::QuestGreeting(QuestGiverList {
            npc: giver,
            ..Default::default()
        });
        rig.packets(vec![greeting], 2);
        let detail = SessionEvent::QuestDetail(QuestDetails {
            npc: giver,
            quest_id: QUEST,
            title: "Charz".into(),
            ..Default::default()
        });
        let seen = rig.packets(vec![detail], 2);
        assert_eq!(
            fired(&seen, &["QUEST_FINISHED", "QUEST_DETAIL"]),
            [vec!["QUEST_FINISHED"; finishes], vec!["QUEST_DETAIL"]].concat(),
            "giver {giver:#x}"
        );
        assert_eq!(
            rig.app
                .world()
                .resource::<crate::ui_session::InteractTarget>()
                .0,
            Some(giver)
        );
    }
}

/// `"npc"` on a GameObject interact target is no unit to the getters: `UnitExists` asks the object's targetable slot (`0x60c2f8`), which a GameObject answers 0 (`0x8e5250`), and `UnitName` reads the template's name, `""` until it is cached (`0x60e838` -> `0x70cdf0`); `UnitGUID` names it.
#[test]
fn npc_on_a_gameobject_names_its_template_and_is_no_unit() {
    const BOX: u64 = 0xF110_0000_0300_0021;
    const BARE: u64 = 0xF110_0000_0301_0022;
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    let (tx, _rx) = crossbeam_channel::unbounded();
    rig.app.insert_resource(crate::net::NetCommands(tx));
    let mailbox = |guid: u64, entry: u32| SessionEvent::ObjectCreate {
        guid,
        kind: EntityKind::GameObject,
        display_id: None,
        position: [-8949.0, -130.0, 83.5],
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
        // `OBJECT_FIELD_TYPE`: OBJECT | GAMEOBJECT; `OBJECT_FIELD_ENTRY`.
        fields: ObjectFields::from_pairs(&[(2, 0x21), (3, entry)])
            .into_created(ObjectType::GameObject),
    };
    rig.packets(vec![mailbox(BOX, 0x300), mailbox(BARE, 0x301)], 2);
    rig.app
        .world_mut()
        .resource_mut::<crate::go_templates::GameObjectTemplates>()
        .insert(0x300, 19, "Mailbox".into(), &[0; 24]);
    lua(
        &mut rig,
        "local f = CreateFrame('Frame') f:RegisterEvent('MAIL_SHOW') \
         f:SetScript('OnEvent', function() STABLE_LOG = STABLE_LOG or {} \
         table.insert(STABLE_LOG, strjoin('|', tostring(UnitName('npc')), \
         tostring(UnitGUID('npc')), tostring(UnitExists('npc')))) end)",
    );
    rig.packets(vec![SessionEvent::ShowMailbox { mailbox: BOX }], 2);
    rig.packets(vec![SessionEvent::ShowMailbox { mailbox: BARE }], 2);
    assert_eq!(
        lua(&mut rig, ""),
        ["Mailbox|0xF110000003000021|nil", "|0xF110000003010022|nil"]
    );
}

// ── The spellbook (#1281) ────────────────────────────────────────────────────────────────────────

const FIREBALL_1: u32 = 133;
const FIREBALL_2: u32 = 143;
const FROSTBOLT_1: u32 = 116;

/// The book update (`0x53ca70`) runs where 12340 runs it, each ending in `SPELLS_CHANGED` with the
/// book rebuilt: our create's init (`0x6e7fdb`); a live learn, its tab flash after it (`0x542cd4`,
/// `0x542d16`); a supersede, the removal's update then the learn's (`0x5401b8`, `0x542cd4`); a
/// removal (`0x5401b8`); and our level watch, which re-sorts nothing (`0x6d66fd`). Fireball and
/// Frostbolt sit on two skill lines, so the learn flashes the second tab. A handler's events fire
/// once its changes apply, so both of the supersede's read the new rank in the book. A hover of the
/// new slot inside the learn's own events shows its whole tooltip: the builder runs at the call.
#[test]
fn book_updates_fire_from_their_handlers_in_wire_order() {
    let Some(mut rig) = Rig::login_after(vec![book(&[FIREBALL_1])], &[(2, 0x19)]) else {
        return;
    };
    let events = &["SPELLS_CHANGED", "LEARNED_SPELL_IN_TAB"];
    let verbs = &["tabs", "book1", "book2", "tip2"];
    let seen = rig.packets(
        vec![
            SessionEvent::ObjectDestroyed(ME),
            self_create(&[(2, 0x19)]),
            SessionEvent::SpellLearned {
                spell_id: FROSTBOLT_1,
            },
            SessionEvent::SpellSuperceded {
                old_spell_id: FIREBALL_1,
                new_spell_id: FIREBALL_2,
            },
            SessionEvent::SpellRemoved {
                spell_id: FROSTBOLT_1,
            },
            self_values(&[(FIELD_UNIT_LEVEL, 2)]),
        ],
        3,
    );
    assert_eq!(
        pin(&seen, events, verbs),
        vec![
            "SPELLS_CHANGED | tabs=1 book1=Fireball book2=nil tip2=nil",
            "SPELLS_CHANGED | tabs=2 book1=Fireball book2=Frostbolt tip2=4:Launches a bolt of frost at the enemy, causing 18 to 20 Frost damage and slowing movement speed by 40% for 5 sec.",
            "LEARNED_SPELL_IN_TAB 2 | tabs=2 book1=Fireball book2=Frostbolt tip2=4:Launches a bolt of frost at the enemy, causing 18 to 20 Frost damage and slowing movement speed by 40% for 5 sec.",
            "SPELLS_CHANGED | tabs=2 book1=Fireball book2=Frostbolt tip2=4:Launches a bolt of frost at the enemy, causing 18 to 20 Frost damage and slowing movement speed by 40% for 5 sec.",
            "SPELLS_CHANGED | tabs=2 book1=Fireball book2=Frostbolt tip2=4:Launches a bolt of frost at the enemy, causing 18 to 20 Frost damage and slowing movement speed by 40% for 5 sec.",
            "LEARNED_SPELL_IN_TAB 1 | tabs=2 book1=Fireball book2=Frostbolt tip2=4:Launches a bolt of frost at the enemy, causing 18 to 20 Frost damage and slowing movement speed by 40% for 5 sec.",
            "SPELLS_CHANGED | tabs=1 book1=Fireball book2=nil tip2=nil",
            "SPELLS_CHANGED | tabs=1 book1=Fireball book2=nil tip2=nil",
        ]
    );
}

/// A party list in the same frame as a handler-fired event: `party1` resolves at the event against
/// the roster the packets before it wrote (`0x52bc50`), in wire order, and not before it lands.
#[test]
fn a_party_token_resolves_at_the_event_against_the_roster_so_far() {
    for party_first in [true, false] {
        let Some(mut rig) = Rig::login(&[]) else {
            return;
        };
        let frame = if party_first {
            vec![party_with(0x50), active_status(489)]
        } else {
            vec![active_status(489), party_with(0x50)]
        };
        let seen = rig.packets(frame, 1);
        let want = if party_first {
            "0x0000000000000050"
        } else {
            "nil"
        };
        assert_eq!(
            pin(&seen, &["UPDATE_BATTLEFIELD_STATUS"], &["p1"]),
            vec![format!("UPDATE_BATTLEFIELD_STATUS 1 | p1={want}")],
            "party first: {party_first}"
        );
        assert_eq!(rig.live_now().line(&["p1"]), "NOW | p1=0x0000000000000050");
    }
}

/// The selection setter's write is the target global a token reads in the same call
/// (`0x524f38`, `0x60acfe`), and a hop off it reads the new target's own target (`0x60aaa0`).
#[test]
fn a_target_token_reads_the_selection_inside_the_call_that_moved_it() {
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    let answer = super::live::with_live_world(rig.app.world_mut(), |s| {
        s.eval::<String>(
            "local before = tostring(UnitGUID('target')) TargetUnit('player') \
             return before .. ' ' .. tostring(UnitGUID('target')) .. ' ' \
             .. tostring(UnitGUID('targettarget')) .. ' ' .. tostring(UnitIsUnit('target', 'player'))",
        )
    })
    .expect("a VM")
    .expect("the chunk runs");
    assert_eq!(answer, "nil 0x0000000000000010 0x0000000000000010 1");
}

/// `vehicle` reads our seat at the call (`0x60ac87`): a seat taken between frames names the
/// vehicle before any feed runs, and leaving it names nobody at once.
#[test]
fn the_vehicle_token_reads_the_seat_at_the_call() {
    const CART: u64 = 0xF150_0000_0000_0001;
    let Some(mut rig) = Rig::login(&[OBJECT_TYPE_PLAYER]) else {
        return;
    };
    let me = rig
        .app
        .world_mut()
        .query_filtered::<Entity, With<crate::net::SelfPlayer>>()
        .single(rig.app.world())
        .expect("our player");
    let seat = crate::vehicle::Passenger {
        vehicle: CART,
        seat: 0,
        state: crate::vehicle::SEATED,
    };
    rig.app.world_mut().entity_mut(me).insert(seat);
    assert_eq!(
        rig.live_now().line(&["veh"]),
        "NOW | veh=0xF150000000000001"
    );
    rig.app
        .world_mut()
        .entity_mut(me)
        .remove::<crate::vehicle::Passenger>();
    assert_eq!(rig.live_now().line(&["veh"]), "NOW | veh=nil");
}
