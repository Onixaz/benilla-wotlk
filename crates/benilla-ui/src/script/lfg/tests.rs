use super::*;
use crate::script::UiScript;

const CLASSIC: i32 = 1;
const WRATH_NORMAL: i32 = 4;
const WRATH_HEROIC: i32 = 5;
const WARRIOR: u32 = 1;
const MAGE: u32 = 8;

fn dungeon(id: i32, name: &str, type_id: i32, group_id: i32, levels: (i32, i32)) -> LfgDungeonView {
    LfgDungeonView {
        id,
        name: name.into(),
        min_level: levels.0,
        max_level: levels.1,
        target_level: levels.1,
        target_level_min: levels.0,
        target_level_max: levels.1,
        map_id: 600 + id,
        difficulty: i32::from(type_id == 5),
        flags: 0x3,
        type_id,
        texture: name.to_uppercase(),
        expansion_level: if group_id == CLASSIC { 0 } else { 2 },
        group_id,
        ..Default::default()
    }
}

fn random(id: i32, name: &str, expansion: i32, levels: (i32, i32)) -> LfgDungeonView {
    LfgDungeonView {
        id,
        name: name.into(),
        min_level: levels.0,
        max_level: levels.1,
        target_level_min: levels.0,
        target_level_max: levels.1,
        flags: 0x3,
        type_id: 6,
        expansion_level: expansion,
        group_id: if expansion == 0 {
            CLASSIC
        } else {
            WRATH_NORMAL
        },
        ..Default::default()
    }
}

fn group(id: i32, name: &str, order_index: i32, type_id: i32) -> LfgGroupView {
    LfgGroupView {
        id,
        name: name.into(),
        order_index,
        parent_group_id: 0,
        type_id,
    }
}

/// Two classic dungeons, a Wrath dungeon and its heroic, two random rows, and a raid.
fn catalog() -> LfgCatalogView {
    LfgCatalogView {
        dungeons: vec![
            dungeon(6, "Deadmines", 1, CLASSIC, (15, 25)),
            dungeon(1, "Wailing Caverns", 1, CLASSIC, (15, 25)),
            dungeon(202, "Utgarde Keep", 1, WRATH_NORMAL, (69, 80)),
            dungeon(242, "Utgarde Keep", 5, WRATH_HEROIC, (80, 80)),
            random(258, "Random Classic Dungeon", 0, (15, 60)),
            random(261, "Random Lich King Dungeon", 2, (69, 80)),
            LfgDungeonView {
                flags: 0,
                ..dungeon(159, "Naxxramas", 2, 9, (80, 80))
            },
        ],
        groups: vec![
            group(WRATH_HEROIC, "Wrath of the Lich King Heroic", 1, 5),
            group(WRATH_NORMAL, "Wrath of the Lich King Normal", 2, 1),
            group(CLASSIC, "Classic Dungeons", 5, 1),
            group(9, "Wrath of the Lich King Raid (25)", 17, 2),
        ],
        expansions: Vec::new(),
        map_difficulties: vec![MapDifficultyView {
            map_id: 802,
            difficulty: 0,
            max_players: 5,
            difficulty_string: "DUNGEON_DIFFICULTY_5PLAYER".into(),
        }],
        encounters: vec![
            EncounterView {
                map_id: 802,
                difficulty: 0,
                bit: 0,
                name: "Prince Keleseth".into(),
                icon: None,
            },
            EncounterView {
                map_id: 802,
                difficulty: 0,
                bit: 1,
                name: "Ingvar".into(),
                icon: None,
            },
        ],
    }
}

/// A script with the three CVars registered, the catalog pushed, the game initialized and an
/// event log.
fn script(class: u32) -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.register_cvars([
        ("lfgSelectedRoles", "0"),
        ("lfdCollapsedHeaders", ""),
        ("lfdSelectedDungeons", ""),
    ]);
    s.set_account_expansion(2);
    s.set_lfg_catalog(catalog());
    s.lfg_initialize_game();
    s.set_lfg_class(class);
    s.run(
        r#"
        LOG = {}
        local f = CreateFrame("Frame")
        for _, e in ipairs({"LFG_UPDATE", "LFG_ROLE_UPDATE", "LFG_PROPOSAL_UPDATE",
            "LFG_PROPOSAL_SHOW", "LFG_PROPOSAL_FAILED", "LFG_PROPOSAL_SUCCEEDED",
            "LFG_LOCK_INFO_RECEIVED", "LFG_UPDATE_RANDOM_INFO", "LFG_ROLE_CHECK_SHOW",
            "LFG_ROLE_CHECK_HIDE", "LFG_ROLE_CHECK_UPDATE", "LFG_ROLE_CHECK_ROLE_CHOSEN",
            "LFG_QUEUE_STATUS_UPDATE"}) do
            f:RegisterEvent(e)
        end
        f:SetScript("OnEvent", function(self, event, a)
            LOG[#LOG + 1] = event .. (a ~= nil and ("=" .. tostring(a)) or "")
        end)
        "#,
    )
    .unwrap();
    s
}

/// The events fired since the last read.
fn log(s: &mut UiScript) -> Vec<String> {
    let v: Vec<String> = s
        .eval::<mlua::Table>("return LOG")
        .unwrap()
        .sequence_values()
        .map(Result::unwrap)
        .collect();
    s.run("LOG = {}").unwrap();
    v
}

fn list(s: &UiScript, chunk: &str) -> Vec<i32> {
    s.eval::<mlua::Table>(chunk)
        .unwrap()
        .sequence_values()
        .map(Result::unwrap)
        .collect()
}

fn slot(id: u32, ty: u32) -> u32 {
    id | ty << 24
}

fn queued() -> LfgUpdateView {
    LfgUpdateView {
        update_type: 5,
        joined: true,
        lfg_joined: true,
        queued: true,
        slots: vec![slot(261, 6)],
        ..Default::default()
    }
}

/// Headers sort by their group order and lead their children, which sort by order index, mean
/// level and name; random and unlisted rows stay out, but every typed header is in, the raid's
/// childless one too, which the stock list drops itself (`0x558350`, `0x5536a0`).
#[test]
fn the_choice_order_heads_each_group_with_its_header() {
    let s = script(WARRIOR);
    assert_eq!(
        list(&s, "return GetLFDChoiceOrder()"),
        [-WRATH_HEROIC, 242, -WRATH_NORMAL, 202, -CLASSIC, 6, 1, -9]
    );
    let raids = list(&s, "return GetLFRChoiceOrder()");
    assert!(raids.contains(&159) && raids.contains(&-9));
}

/// A dungeon's info carries its negated group, the map's player cap and the seasonal flag; a
/// header's its name and type; a passed table is cleared and returned.
#[test]
fn the_choice_info_fills_dungeons_and_headers() {
    let s = script(WARRIOR);
    let (name, group, size, seasonal, header): (String, i32, i32, bool, String) = s
        .eval("local t = GetLFDChoiceInfo() local d = t[202] return d[1], d[9], d[12], d[14], t[-4][1]")
        .unwrap();
    assert_eq!(
        (name.as_str(), group, size, seasonal),
        ("Utgarde Keep", -WRATH_NORMAL, 5, false)
    );
    assert_eq!(header, "Wrath of the Lich King Normal");
    let reused: bool = s
        .eval(
            "local t = {stale = 1} local r = GetLFDChoiceInfo(t) return r == t and t.stale == nil",
        )
        .unwrap();
    assert!(reused);
}

/// The roles a class cannot take are dropped (`0x553d60`), and the change fires.
#[test]
fn roles_keep_only_the_classes_own() {
    let mut s = script(MAGE);
    s.run("SetLFGRoles(true, true, true, true)").unwrap();
    let roles: (bool, bool, bool, bool) = s.eval("return GetLFGRoles()").unwrap();
    assert_eq!(
        roles,
        (true, false, false, true),
        "a mage leads and deals damage"
    );
    assert_eq!(log(&mut s), ["LFG_ROLE_UPDATE"]);
    let avail: (bool, bool, bool) = s.eval("return GetAvailableRoles()").unwrap();
    assert_eq!(avail, (false, false, true));
}

/// Selections and collapses persist as bit-array CVars, a dungeon reading its group's bit.
#[test]
fn the_selection_and_collapse_states_live_in_their_cvars() {
    let s = script(WARRIOR);
    s.run("SetLFGDungeonEnabled(6) SetLFGDungeonEnabled(1, false) SetLFGHeaderCollapsed(-1, true)")
        .unwrap();
    let (six, one, header_closed, child_closed): (bool, Option<bool>, bool, bool) = s
        .eval(
            "local e = GetLFDChoiceEnabledState() local c = GetLFDChoiceCollapseState() \
             return e[6], e[1], c[-1], c[6]",
        )
        .unwrap();
    assert!(six && one.is_none());
    assert!(header_closed && child_closed);
    assert!(s.cvar("lfdSelectedDungeons").is_some_and(|v| !v.is_empty()));
}

/// No dungeon or no role refuses with a line; a valid selection sends the join once.
#[test]
fn join_refuses_an_empty_selection_and_sends_a_valid_one() {
    let mut s = script(WARRIOR);
    s.run("JoinLFG()").unwrap();
    assert_eq!(s.take_ui_errors(), ["ERR_LFG_NO_SLOTS_SELECTED"]);
    s.run("SetLFGDungeon(261) JoinLFG()").unwrap();
    assert_eq!(s.take_ui_errors(), ["ERR_LFG_NO_ROLES_SELECTED"]);
    assert!(s.take_lfg_requests().is_empty());
    s.run("SetLFGRoles(false, true, false, false) JoinLFG() JoinLFG()")
        .unwrap();
    assert_eq!(
        s.take_lfg_requests(),
        [LfgRequest::Join {
            roles: 0x2,
            no_partial_clear: false,
            achievements: false,
            slots: vec![slot(261, 6)],
            needs: [0; 3],
            comment: String::new(),
        }],
        "joined and unchanged, the second call sends nothing"
    );
}

/// A dungeon joins a dungeon list, a raid starts a raid list (`0x559b70`).
#[test]
fn set_dungeon_keeps_one_kind_of_list() {
    let s = script(WARRIOR);
    s.run("SetLFGDungeon(6) SetLFGDungeon(242)").unwrap();
    assert_eq!(
        s.eval::<i32>("return select(5, GetLFGInfoLocal())")
            .unwrap(),
        2
    );
    s.run("SetLFGDungeon(159)").unwrap();
    assert_eq!(
        s.eval::<i32>("return select(5, GetLFGInfoLocal())")
            .unwrap(),
        1
    );
}

/// The lock info lists random rewards and locks; the best random skips a locked one.
#[test]
fn player_info_builds_the_random_list_and_its_rewards() {
    let mut s = script(WARRIOR);
    s.lfg_player_info(
        vec![LfgDungeonRewardView {
            slot: slot(261, 6),
            done: true,
            base_money: 100,
            base_xp: 200,
            items: vec![LfgRewardItemView {
                item: 29434,
                display: 7,
                count: 2,
            }],
            ..Default::default()
        }],
        vec![(slot(258, 6), 1), (slot(202, 1), 2)],
    );
    assert_eq!(
        log(&mut s),
        ["LFG_LOCK_INFO_RECEIVED", "LFG_UPDATE_RANDOM_INFO"]
    );
    let (n, best, id): (i32, i32, i32) = s
        .eval("return GetNumRandomDungeons(), GetRandomDungeonBestChoice(), GetLFGRandomDungeonInfo(2)")
        .unwrap();
    assert_eq!((n, best, id), (2, 261, 261));
    let (done, money, items): (bool, i32, i32) = s
        .eval("local d, m, _, _, _, n = GetLFGDungeonRewards(261) return d, m, n")
        .unwrap();
    assert_eq!((done, money, items), (true, 100, 1));
    let (name, count): (String, i32) = s
        .eval("local n, _, c = GetLFGDungeonRewardInfo(261, 1) return n, c")
        .unwrap();
    assert_eq!(
        (name.as_str(), count),
        ("", 2),
        "the name waits for the template"
    );
    assert_eq!(s.take_item_stat_asks(), [29434]);
    let (lock, reason): (i32, i32) = s
        .eval(
            "local t = GetLFDChoiceLockedState() return t[202], select(2, GetLFDLockInfo(202, 1))",
        )
        .unwrap();
    assert_eq!((lock, reason), (2, 2));
    assert!(!s.eval::<bool>("return IsLFGDungeonJoinable(258)").unwrap());
}

/// A fresh join shows its line and chime; an unchanged repeat shows nothing; a leave by update
/// type 7 shows the left line.
#[test]
fn player_updates_announce_joins_and_leaves() {
    let mut s = script(WARRIOR);
    let lines = s.lfg_update_player(queued(), false);
    assert_eq!(
        lines,
        [LfgLine::Sound(0x210e), LfgLine::Key("ERR_LFG_JOINED_QUEUE")]
    );
    assert_eq!(log(&mut s), ["LFG_UPDATE=false"]);
    let (grouped, j, q, n): (bool, bool, bool, i32) = s
        .eval("local p, j, q, _, _, _, n = GetLFGInfoServer() return p, j, q, n")
        .unwrap();
    assert_eq!((grouped, j, q, n), (false, true, true, 1));
    assert!(s.lfg_update_player(queued(), false).is_empty());
    assert!(log(&mut s).is_empty(), "an unchanged update fires nothing");
    let lines = s.lfg_update_player(
        LfgUpdateView {
            update_type: 7,
            ..Default::default()
        },
        false,
    );
    assert_eq!(
        lines,
        [LfgLine::Sound(0x43bd), LfgLine::Key("ERR_LFG_LEFT_QUEUE")]
    );
}

/// The waits come back in the stock order: average, tank, healer, damage, mine.
#[test]
fn queue_stats_answer_while_queued() {
    let mut s = script(WARRIOR);
    s.lfg_update_player(queued(), false);
    s.set_now(100.0);
    s.lfg_queue_status(LfgQueueStatusView {
        dungeon: 261,
        my_wait: 10,
        average_wait: 20,
        tank_wait: 30,
        healer_wait: 40,
        damage_wait: 50,
        tanks_needed: 1,
        healers_needed: 1,
        damage_needed: 3,
        queued_secs: 30,
    });
    let v: (bool, i32, i32, String, i32, i32, i32, i32, i32, f64) = s
        .eval(
            "local h, _, t, _, _, ty, name, avg, tw, hw, dw, mw, at = GetLFGQueueStats() \
             return h, t, ty, name, avg, tw, hw, dw, mw, at",
        )
        .unwrap();
    assert_eq!(
        v,
        (
            true,
            1,
            6,
            "Random Lich King Dungeon".into(),
            20,
            30,
            40,
            50,
            10,
            70.0
        )
    );
}

/// A new proposal shows once; our accept marks our row and sends; a party decline fails it.
#[test]
fn a_proposal_shows_once_and_answers() {
    let mut s = script(WARRIOR);
    let proposal = LfgProposalView {
        slot: slot(202, 1),
        state: 0,
        id: 7,
        completed_mask: 0b1,
        silent: false,
        members: vec![
            LfgProposalMemberView {
                roles: 0x2,
                is_local: true,
                ..Default::default()
            },
            LfgProposalMemberView {
                roles: 0x8,
                ..Default::default()
            },
        ],
    };
    assert!(s.lfg_proposal(proposal.clone()).is_empty());
    assert_eq!(log(&mut s), ["LFG_PROPOSAL_UPDATE", "LFG_PROPOSAL_SHOW"]);
    s.lfg_proposal(proposal.clone());
    assert_eq!(
        log(&mut s),
        ["LFG_PROPOSAL_UPDATE"],
        "the same id shows no second time"
    );
    let (exists, id, name, role, total, done): (bool, i32, String, String, i32, i32) = s
        .eval("local e, _, id, n, _, r, _, t, d = GetLFGProposal() return e, id, n, r, t, d")
        .unwrap();
    assert_eq!(
        (exists, id, name.as_str(), role.as_str(), total, done),
        (true, 202, "Utgarde Keep", "TANK", 2, 1)
    );
    let boss: (String, bool) = s
        .eval("local n, _, k = GetLFGProposalEncounter(1) return n, k")
        .unwrap();
    assert_eq!(boss, ("Prince Keleseth".into(), true));
    s.run("AcceptProposal()").unwrap();
    assert_eq!(
        s.take_lfg_requests(),
        [LfgRequest::ProposalResult {
            id: 7,
            accept: true
        }]
    );
    let answered: (bool, bool) = s
        .eval("local _, _, _, r, a = GetLFGProposalMember(1) return r, a")
        .unwrap();
    assert_eq!(answered, (true, true));
    log(&mut s);
    let mut declined = proposal;
    declined.members[1].answered = true;
    declined.members[1].same_group = true;
    let lines = s.lfg_proposal(declined);
    assert_eq!(
        lines,
        [
            LfgLine::Sound(0x43bd),
            LfgLine::Key("ERR_LFG_PROPOSAL_DECLINED_PARTY")
        ]
    );
    assert_eq!(log(&mut s), ["LFG_PROPOSAL_UPDATE", "LFG_PROPOSAL_FAILED"]);
    assert!(s
        .eval::<Option<bool>>("return GetLFGProposal()")
        .unwrap()
        .is_none());
}

/// A role check in progress shows for us until we confirm; an abort hides it with a line.
#[test]
fn a_role_check_shows_until_confirmed() {
    let mut s = script(WARRIOR);
    let me = 1;
    let check = |state, confirmed| LfgRoleCheckView {
        state,
        beginning: false,
        slots: vec![slot(261, 6)],
        members: vec![LfgRoleCheckMemberView {
            guid: me,
            confirmed,
            roles: 0x3,
            level: 80,
        }],
    };
    s.lfg_role_check(check(2, false), me, &|_| None);
    assert_eq!(
        log(&mut s),
        ["LFG_ROLE_CHECK_UPDATE", "LFG_ROLE_CHECK_SHOW"]
    );
    let (on, slots, members): (bool, i32, i32) = s.eval("return GetLFGRoleUpdate()").unwrap();
    assert_eq!((on, slots, members), (true, 1, 1));
    let lines = s.lfg_role_check(check(5, true), me, &|_| None);
    assert_eq!(
        lines,
        [
            LfgLine::Sound(0x43bd),
            LfgLine::Key("ERR_LFG_ROLE_CHECK_ABORTED")
        ]
    );
    assert_eq!(
        log(&mut s),
        ["LFG_ROLE_CHECK_UPDATE", "LFG_ROLE_CHECK_HIDE"]
    );
}

/// The teleport refuses in combat with its line and sends otherwise.
#[test]
fn the_teleport_gates_on_the_player() {
    let mut s = script(WARRIOR);
    let player = |in_combat| crate::script::UnitState {
        exists: true,
        in_combat,
        ..Default::default()
    };
    s.set_unit("player", Some(player(true)));
    assert!(!s.eval::<bool>("return LFGTeleport(false)").unwrap());
    assert_eq!(s.take_ui_errors(), ["ERR_NOT_IN_COMBAT"]);
    s.set_unit("player", Some(player(false)));
    assert!(s.eval::<bool>("return LFGTeleport(true)").unwrap());
    assert_eq!(s.take_lfg_requests(), [LfgRequest::Teleport(true)]);
}

/// The join result picks its line from `0xacdda8`; result 6 keeps the party's locks.
#[test]
fn join_results_pick_their_lines() {
    let mut s = script(WARRIOR);
    assert_eq!(
        s.lfg_join_result(1, 3, None),
        [LfgLine::Key("ERR_LFG_ROLE_CHECK_FAILED_TIMEOUT")]
    );
    assert_eq!(
        s.lfg_join_result(12, 0, None),
        [LfgLine::Key("ERR_LFG_DESERTER_PLAYER")]
    );
    let lines = s.lfg_join_result(
        6,
        0,
        Some(vec![LfgMemberLocksView {
            guid: 9,
            locks: vec![(slot(6, 1), 1)],
        }]),
    );
    assert_eq!(lines, [LfgLine::Key("ERR_LFG_NO_SLOTS_PARTY")]);
    assert_eq!(
        s.eval::<i32>("return GetLFDChoiceLockedState()[6]")
            .unwrap(),
        1
    );
}

/// A dungeon instance answers its `MapDifficulty` row's cap.
#[test]
fn instance_info_reads_its_map_difficulty() {
    let mut s = script(WARRIOR);
    s.set_lfg_world(802, 0, 0);
    let (difficulty, size): (i32, i32) = s
        .eval("local _, _, d, _, n = GetInstanceInfo() return d, n")
        .unwrap();
    assert_eq!((difficulty, size), (1, 5));
}
