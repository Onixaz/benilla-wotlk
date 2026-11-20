//! The gossip bindings. Every verb reads the gossip globals at its call through the lent view
//! ([`GossipRead`]): the greeting the last drawn text wrote (`0xc00ef0`), the options and quest rows
//! the last `SMSG_GOSSIP_MESSAGE` read (`0xc00bf0`, `0xbfc968`) and the text-pending latch
//! (`0xc00be8`). None of them test the NPC: a close leaves them as they were. `SelectGossipOption`,
//! the two quest selects and `CloseGossip` send and close inside their call.

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::{bool_or_default, flag, number_arg, optional_string};
use super::ScriptValue;

/// One gossip option; its 1-based index is its place in [`GossipRead::options`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GossipOptionView {
    /// The option's label.
    pub label: String,
    /// The lowercase icon type (`"gossip"`, `"vendor"`, `"taxi"`, …); `GossipFrame.lua` draws
    /// `Interface\GossipFrame\<Type>GossipIcon`.
    pub icon_type: String,
    /// A select asks for a code first (`GOSSIP_ENTER_CODE`).
    pub coded: bool,
    /// The confirmation asked before a select (`GOSSIP_CONFIRM`), `None` for none.
    pub box_text: Option<String>,
    /// Copper the select costs, shown in the confirmation and checked against the purse.
    pub box_money: u32,
}

/// One quest row of `SMSG_GOSSIP_MESSAGE`; a click sends a questgiver query or a turn-in.
/// `active`, from the wire icon, puts it under current quests rather than available ones.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GossipQuestRow {
    /// Never returned to Lua; `GetGossipActiveQuests` reads the log's completion by it.
    pub quest_id: u32,
    pub title: String,
    /// -1 for a quest that scales to the player, never trivial.
    pub level: i32,
    pub active: bool,
    /// The template's daily flag (`0x1000`, `0x58b423`).
    pub daily: bool,
    pub repeatable: bool,
}

/// The gossip globals at the call.
#[derive(Clone, Copy, Debug)]
pub struct GossipRead<'a> {
    /// The last drawn greeting (`0xc00ef0`), empty before the first.
    pub greeting: &'a str,
    pub options: &'a [GossipOptionView],
    pub quests: &'a [GossipQuestRow],
    /// The text-pending latch (`0xc00be8`), under which `SelectGossipOption` does nothing.
    pub pending: bool,
    /// `ForceGossip` (`0x58ab50`): the held NPC's creature type flag `0x8000000`.
    pub force_gossip: bool,
    /// Our coinage, which a select's box money is held against (`0x58afcd`).
    pub money: u64,
}

/// `f` over the gossip globals at the call; `None` with no view lent.
fn read<R>(f: impl FnOnce(&GossipRead<'_>) -> R) -> Option<R> {
    super::read_live(|l| l.gossip().map(|g| f(&g))).flatten()
}

/// The quest-trivial test (`0x58aba0`): a quest is grey when its level plus the player's grey
/// margin falls below the player's level. The margin is the table at `0xa12fd0`, indexed by
/// `level / 5` and 8 past its end: 4 below level 10, one more per ten levels, at most 8.
pub(super) fn quest_is_trivial(quest_level: i32, player_level: u32) -> bool {
    if quest_level == -1 {
        return false;
    }
    let player = player_level as i64;
    let margin = (4 + player / 10).min(8);
    i64::from(quest_level) + margin < player
}

/// Register the gossip globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // `0x58a900`: the greeting buffer as it is.
    g.set(
        "GetGossipText",
        lua.create_function(|lua, ()| {
            let text = read(|g| g.greeting.to_owned()).unwrap_or_default();
            lua.create_string(text)
        })?,
    )?;

    // `0x58a9e0`: `(text, type)` per option.
    g.set(
        "GetGossipOptions",
        lua.create_function(|lua, ()| {
            let pairs: Vec<(String, String)> = read(|g| {
                g.options
                    .iter()
                    .map(|o| (o.label.clone(), o.icon_type.clone()))
                    .collect()
            })
            .unwrap_or_default();
            let mut out: Vec<Value> = Vec::with_capacity(pairs.len() * 2);
            for (label, icon) in pairs {
                out.push(Value::String(lua.create_string(&label)?));
                out.push(Value::String(lua.create_string(&icon)?));
            }
            Ok(MultiValue::from_vec(out))
        })?,
    )?;

    // The three counts (`0x58a920`, `0x58a960`, `0x58a9a0`).
    fn install_count(lua: &Lua, name: &str, count: fn(&GossipRead) -> usize) -> mlua::Result<()> {
        lua.globals().set(
            name,
            lua.create_function(move |_, ()| Ok(read(count).unwrap_or(0) as i64))?,
        )
    }
    install_count(lua, "GetNumGossipOptions", |g| g.options.len())?;
    install_count(lua, "GetNumGossipAvailableQuests", |g| {
        g.quests.iter().filter(|q| !q.active).count()
    })?;
    install_count(lua, "GetNumGossipActiveQuests", |g| {
        g.quests.iter().filter(|q| q.active).count()
    })?;

    // ForceGossip() (`0x58ab50`): a Lua boolean, false with no NPC.
    g.set(
        "ForceGossip",
        lua.create_function(|_, ()| Ok(read(|g| g.force_gossip).unwrap_or(false)))?,
    )?;

    // SelectGossipOption(index [, code [, confirmed]]) (`0x58b590` -> `0x58af10`).
    g.set(
        "SelectGossipOption",
        lua.create_function(|lua, args: MultiValue| {
            let mut args = args.into_iter();
            let index = number_arg(
                lua,
                args.next().unwrap_or(Value::Nil),
                "Usage: SelectGossipOption(index)",
            )?;
            let code = args.next().and_then(|v| optional_string(lua, &v));
            let confirmed = bool_or_default(args.next().as_ref(), false);
            select_option(lua, index, code, confirmed);
            Ok(())
        })?,
    )?;

    // `0x58aa40` -> `0x58a550`: each close fires `GOSSIP_CLOSED` inside the call; the interaction close re-enters it once while the NPC holds the interact target (`0x58a588` -> `0x512ef6`).
    g.set(
        "CloseGossip",
        lua.create_function(|lua, ()| {
            let closes = super::write_live(|l| l.close_gossip()).unwrap_or(0);
            for _ in 0..closes {
                super::tick::fire_event_into(lua, "GOSSIP_CLOSED", Vec::new());
            }
            Ok(())
        })?,
    )?;

    // ══ The two quest lists ══
    // The reference splits the quest rows by wire icon, 3 and 4 active and the rest available
    // (`0x58a5d0`, `0x58a6c0`). An available row is `(title, level, isTrivial, isDaily,
    // isRepeatable)` (`0x58b3a0`), an active one `(title, level, isTrivial, isComplete)`
    // (`0x58b490`); the flags are 1 or nil.
    fn quest_rows(lua: &Lua, active: bool) -> mlua::Result<MultiValue> {
        let rows: Vec<GossipQuestRow> = read(|g| {
            g.quests
                .iter()
                .filter(|q| q.active == active)
                .cloned()
                .collect()
        })
        .unwrap_or_default();
        let player_level = crate::script::item_stats::player_level();
        let rows: Vec<(String, i32, [bool; 3])> = {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            rows.into_iter()
                .map(|q| {
                    let trivial = quest_is_trivial(q.level, player_level);
                    let flags = if active {
                        let complete = super::quest_log::quest_complete_at(lua, &model, q.quest_id);
                        [trivial, complete, false]
                    } else {
                        [trivial, q.daily, q.repeatable]
                    };
                    (q.title, q.level, flags)
                })
                .collect()
        };
        let width = if active { 4 } else { 5 };
        let mut out = Vec::with_capacity(rows.len() * width);
        for (title, level, flags) in rows {
            out.push(Value::String(lua.create_string(&title)?));
            out.push(Value::Integer(i64::from(level)));
            out.extend(flags[..width - 2].iter().map(|&f| flag(f)));
        }
        Ok(MultiValue::from_vec(out))
    }

    g.set(
        "GetGossipAvailableQuests",
        lua.create_function(|lua, ()| quest_rows(lua, false))?,
    )?;
    g.set(
        "GetGossipActiveQuests",
        lua.create_function(|lua, ()| quest_rows(lua, true))?,
    )?;

    // A select's index is 1-based within its own list (`0x58b070`, `0x58b130`); the app sends for the row at its place in the whole menu, inside the call.
    fn select_quest(active: bool, index: i32) {
        let Some(nth) = usize::try_from(index.wrapping_sub(1)).ok() else {
            return;
        };
        let whole = read(|g| {
            g.quests
                .iter()
                .enumerate()
                .filter(|(_, q)| q.active == active)
                .nth(nth)
                .map(|(n, _)| n)
        })
        .flatten();
        if let Some(n) = whole {
            super::write_live(|l| l.select_gossip_quest(n));
        }
    }

    g.set(
        "SelectGossipAvailableQuest",
        lua.create_function(|lua, i: Value| {
            let i = number_arg(lua, i, "Usage: SelectGossipAvailableQuest(index)")?;
            select_quest(false, i);
            Ok(())
        })?,
    )?;
    g.set(
        "SelectGossipActiveQuest",
        lua.create_function(|lua, i: Value| {
            let i = number_arg(lua, i, "Usage: SelectGossipActiveQuest(index)")?;
            select_quest(true, i);
            Ok(())
        })?,
    )?;

    Ok(())
}

/// The select sender's checks, in its order (`0x58af10`): nothing while the greeting is pending
/// or for an index outside the menu; an unconfirmed box text asks (`0x58af74`), a coded option
/// with no code asks for one (`0x58afa5`), and box money past the purse refuses (`0x58afcd`).
/// A sent select empties the options and quests (`0x58a7b0`) and fires `GOSSIP_CONFIRM_CANCEL`
/// (`0x58b051`), each inside the call; the greeting stays.
fn select_option(lua: &Lua, index: i32, code: Option<String>, confirmed: bool) {
    let Some(slot) = usize::try_from(index.wrapping_sub(1)).ok() else {
        return;
    };
    let Some(Some((opt, money))) = read(|g| {
        (!g.pending)
            .then(|| g.options.get(slot).map(|o| (o.clone(), g.money)))
            .flatten()
    }) else {
        return;
    };
    if let (Some(text), false) = (&opt.box_text, confirmed) {
        let args = vec![
            ScriptValue::Int(i64::from(index)),
            ScriptValue::Str(text.clone()),
            ScriptValue::Int(i64::from(opt.box_money)),
        ];
        super::tick::fire_event_into(lua, "GOSSIP_CONFIRM", args);
        return;
    }
    let code = code.filter(|c| !c.is_empty());
    if opt.coded && code.is_none() {
        super::tick::fire_event_into(
            lua,
            "GOSSIP_ENTER_CODE",
            vec![ScriptValue::Int(i64::from(index))],
        );
        return;
    }
    if u64::from(opt.box_money) > money {
        super::ui_errors::show_now(lua, "ERR_NOT_ENOUGH_MONEY");
        return;
    }
    let code = if opt.coded { code } else { None };
    super::write_live(|l| l.select_gossip_option(slot, code));
    super::tick::fire_event_into(lua, "GOSSIP_CONFIRM_CANCEL", Vec::new());
}

#[cfg(test)]
mod tests {
    use super::{quest_is_trivial, GossipOptionView, GossipQuestRow};
    use crate::script::live::{TestGossip, TestView};
    use crate::script::UiScript;

    fn option(label: &str, icon: &str) -> GossipOptionView {
        GossipOptionView {
            label: label.into(),
            icon_type: icon.into(),
            ..Default::default()
        }
    }

    fn view() -> TestView {
        TestView {
            gossip: Some(TestGossip {
                greeting: "Greetings, traveler.".into(),
                options: vec![
                    option("Let me browse your goods.", "vendor"),
                    option("I would like to sign the petition.", "gossip"),
                ],
                money: 1_000,
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    /// A chunk that logs `events`' firings, each with its arguments, around `body`.
    fn logged(events: &[&str], body: &str) -> String {
        let reg: String = events
            .iter()
            .map(|e| format!("f:RegisterEvent('{e}') "))
            .collect();
        format!(
            "local f = CreateFrame('Frame') local log = {{}} {reg} \
             f:SetScript('OnEvent', function(_, e, ...) local t = {{e}}              for i = 1, select('#', ...) do t[#t + 1] = tostring((select(i, ...))) end              log[#log + 1] = table.concat(t, ' ') end) \
             {body} f:UnregisterAllEvents() return table.concat(log, '|')"
        )
    }

    const EVENTS: &[&str] = &[
        "GOSSIP_CONFIRM",
        "GOSSIP_ENTER_CODE",
        "GOSSIP_CONFIRM_CANCEL",
        "GOSSIP_CLOSED",
        "UI_ERROR_MESSAGE",
    ];

    #[test]
    fn the_verbs_read_the_globals_at_the_call_and_a_select_sends_in_it() {
        let mut s = UiScript::new().unwrap();
        assert_eq!(s.eval::<String>("return GetGossipText()").unwrap(), "");
        assert_eq!(s.arity("GetGossipOptions()").unwrap(), 0);
        let mut v = view();
        let log = s.with_live(&mut v, |s| {
            assert_eq!(
                s.eval::<String>("return GetGossipText()").unwrap(),
                "Greetings, traveler."
            );
            assert_eq!(s.eval::<i64>("return GetNumGossipOptions()").unwrap(), 2);
            assert_eq!(s.arity("GetGossipOptions()").unwrap(), 4);
            let (l1, t1) = s
                .eval::<(String, String)>("return GetGossipOptions()")
                .unwrap();
            assert_eq!(
                (l1.as_str(), t1.as_str()),
                ("Let me browse your goods.", "vendor")
            );
            // A code on an uncoded option is dropped; the select empties the menu inside the call.
            s.eval::<String>(&logged(
                EVENTS,
                "SelectGossipOption(2, 'unused-code') log[#log + 1] = GetNumGossipOptions() \
                 SelectGossipOption(1)",
            ))
            .unwrap()
        });
        assert_eq!(
            log, "GOSSIP_CONFIRM_CANCEL|0",
            "a spent menu sends nothing more"
        );
        let g = v.gossip.as_ref().unwrap();
        assert_eq!(g.selects, vec![(1, None)]);
        assert_eq!(g.greeting, "Greetings, traveler.", "the greeting stays");
    }

    #[test]
    fn select_asks_for_confirmation_then_a_code_then_the_money_inside_its_call() {
        let mut s = UiScript::new().unwrap();
        let mut v = view();
        let g = v.gossip.as_mut().unwrap();
        g.options = vec![GossipOptionView {
            label: "Open the vault.".into(),
            icon_type: "gossip".into(),
            coded: true,
            box_text: Some("Are you sure?".into()),
            box_money: 500,
        }];
        g.money = 499;
        let log = s.with_live(&mut v, |s| {
            s.eval::<String>(&logged(
                EVENTS,
                "ERR_NOT_ENOUGH_MONEY = \"You don't have enough money.\"                  SelectGossipOption(1) SelectGossipOption(1, '', true) \
                 SelectGossipOption(1, '1234', true)",
            ))
            .unwrap()
        });
        assert_eq!(
            log,
            "GOSSIP_CONFIRM 1 Are you sure? 500|GOSSIP_ENTER_CODE 1|UI_ERROR_MESSAGE You don't have enough money."
        );
        assert!(v.gossip.as_ref().unwrap().selects.is_empty());
        v.gossip.as_mut().unwrap().money = 500;
        s.with_live(&mut v, |s| {
            s.run("SelectGossipOption(1, '1234', 1)").unwrap()
        });
        assert_eq!(
            v.gossip.as_ref().unwrap().selects,
            vec![(0, Some("1234".into()))]
        );
    }

    #[test]
    fn select_is_refused_while_the_greeting_is_pending_or_out_of_range() {
        let mut s = UiScript::new().unwrap();
        let mut v = view();
        v.gossip.as_mut().unwrap().pending = true;
        let log = s.with_live(&mut v, |s| {
            s.eval::<String>(&logged(EVENTS, "SelectGossipOption(1)"))
                .unwrap()
        });
        assert_eq!(log, "");
        v.gossip.as_mut().unwrap().pending = false;
        let log = s.with_live(&mut v, |s| {
            assert!(s.run("SelectGossipOption()").is_err(), "Usage: raises");
            s.eval::<String>(&logged(
                EVENTS,
                "SelectGossipOption(0) SelectGossipOption(3)",
            ))
            .unwrap()
        });
        assert_eq!(log, "");
        assert!(v.gossip.as_ref().unwrap().selects.is_empty());
    }

    #[test]
    fn close_fires_once_per_close_inside_its_call() {
        let mut s = UiScript::new().unwrap();
        let mut v = view();
        v.gossip.as_mut().unwrap().closes = 2;
        let log = s.with_live(&mut v, |s| {
            s.eval::<String>(&logged(
                EVENTS,
                "CloseGossip() log[#log + 1] = 'after' CloseGossip()",
            ))
            .unwrap()
        });
        assert_eq!(log, "GOSSIP_CLOSED|GOSSIP_CLOSED|after");
    }

    #[test]
    fn the_two_gossip_quest_lists_split_by_active_and_select_within_themselves() {
        let mut s = UiScript::new().unwrap();
        // No menu: `arg.n == 0`, not a nil.
        assert_eq!(s.arity("GetGossipAvailableQuests()").unwrap(), 0);
        assert_eq!(s.arity("GetGossipActiveQuests()").unwrap(), 0);
        let mut v = view();
        v.gossip.as_mut().unwrap().quests = vec![
            GossipQuestRow {
                quest_id: 60,
                title: "Report to Goldshire".into(),
                level: 5,
                active: true,
                ..Default::default()
            },
            GossipQuestRow {
                quest_id: 61,
                title: "A Threat Within".into(),
                level: 7,
                daily: true,
                ..Default::default()
            },
            GossipQuestRow {
                quest_id: 62,
                title: "Kobold Camp Cleanup".into(),
                level: 9,
                repeatable: true,
                ..Default::default()
            },
        ];
        s.set_player_req_state(crate::script::PlayerReqState {
            level: 10,
            ..Default::default()
        });
        // Quest 60 is complete in the log, which `isComplete` reads (`0x58bcb0`).
        s.seed_quest_log(crate::script::QuestLogState {
            entries: vec![crate::script::QuestLogEntryView {
                quest_id: 60,
                title: "Report Back".into(),
                complete: 1,
                ..Default::default()
            }],
            ..Default::default()
        });
        s.with_live(&mut v, |s| {
            assert_eq!(
                s.eval::<i64>("return GetNumGossipAvailableQuests()")
                    .unwrap(),
                2
            );
            assert_eq!(s.eval::<i64>("return GetNumGossipActiveQuests()").unwrap(), 1);
            // Five per available row: level 7 at 10 is not trivial (7 + 5 = 12).
            assert_eq!(s.arity("GetGossipAvailableQuests()").unwrap(), 10);
            assert!(s
                .eval::<bool>(
                    "local t,l,triv,daily,rep, t2,l2,triv2,daily2,rep2 = GetGossipAvailableQuests()\n\
                     return t == 'A Threat Within' and l == 7 and triv == nil and daily == 1\n\
                        and rep == nil and t2 == 'Kobold Camp Cleanup' and l2 == 9 and daily2 == nil\n\
                        and rep2 == 1"
                )
                .unwrap());
            assert_eq!(s.arity("GetGossipActiveQuests()").unwrap(), 4);
            assert!(s
                .eval::<bool>(
                    "local t,l,triv,done = GetGossipActiveQuests()\n\
                     return t == 'Report to Goldshire' and l == 5 and triv == nil and done == 1"
                )
                .unwrap());
            // Available #2 is the menu's third row.
            s.run("SelectGossipAvailableQuest(2) SelectGossipActiveQuest(1)")
                .unwrap();
            s.run("SelectGossipAvailableQuest(9) SelectGossipActiveQuest(0)")
                .unwrap();
        });
        assert_eq!(v.gossip.as_ref().unwrap().quest_selects, vec![2, 0]);
    }

    #[test]
    fn a_quest_is_trivial_below_the_players_grey_margin() {
        // Below level 10 the margin is 4; from 40 it is 8; a scaling quest never greys.
        assert!(!quest_is_trivial(1, 5));
        assert!(quest_is_trivial(1, 6));
        assert!(!quest_is_trivial(5, 10));
        assert!(quest_is_trivial(4, 10));
        assert!(!quest_is_trivial(72, 80));
        assert!(quest_is_trivial(71, 80));
        assert!(!quest_is_trivial(-1, 80));
    }

    #[test]
    fn force_gossip_reads_the_flag_at_the_call() {
        let mut s = UiScript::new().unwrap();
        assert!(!s.eval::<bool>("return ForceGossip()").unwrap());
        let mut v = view();
        v.gossip.as_mut().unwrap().force_gossip = true;
        assert!(s.with_live(&mut v, |s| s.eval::<bool>("return ForceGossip()").unwrap()));
    }
}
