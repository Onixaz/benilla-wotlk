//! The gossip bindings: the app pushes the open menu ([`UiScript::set_gossip`]), and
//! `SelectGossipOption` and `CloseGossip` queue intents it drains.
//!
//! `GetGossipOptions()` returns flat `(text, type)` pairs, `type` the lowercase icon name the app
//! maps from the wire icon. `SelectGossipOption(index, code, confirmed)` runs the client's checks
//! before it queues a select (`0x58af10`): an unconfirmed option with box text fires
//! `GOSSIP_CONFIRM`, a coded option without a code fires `GOSSIP_ENTER_CODE`, and box money past
//! the purse shows `ERR_NOT_ENOUGH_MONEY`; the stock popups (`StaticPopup.lua`) call back with
//! `confirmed` set.
//!
//! A menu is pushed only once its greeting (`SMSG_NPC_TEXT_UPDATE`) has arrived; until then the
//! VM keeps its last menu, as the reference's frame keeps its last paint: its text setter returns
//! on a cache miss without an event, and the greeting write and `GOSSIP_SHOW` share one success
//! path (`0x58ac30`).

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::{bool_or_default, flag, number_arg, optional_string};
use super::ScriptValue;

/// One gossip option; its 1-based index is its place in [`GossipMenu::options`].
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

/// One open gossip menu, pushed whole by the app.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GossipMenu {
    /// The NPC greeting, always resolved.
    pub greeting: String,
    /// Quest rows from the same packet.
    pub quests: Vec<GossipQuestRow>,
    pub options: Vec<GossipOptionView>,
    /// `ForceGossip` (`0x58ab50`): the NPC's creature type flag `0x8000000`, which keeps the
    /// stock frame from skipping a lone service option.
    pub force_gossip: bool,
}

impl super::UiScript {
    /// Push (or clear, with `None`) the open gossip menu.
    pub fn set_gossip(&mut self, menu: Option<GossipMenu>) {
        set_field!(self, gossip, menu);
    }

    /// Whether the open menu's greeting query is out: the reference's latch (`0xc00be8`), under
    /// which `SelectGossipOption` does nothing.
    pub fn set_gossip_pending(&mut self, pending: bool) {
        set_field!(self, gossip_pending, pending);
    }

    /// Drain the selects queued since the last call: the 1-based option position, and the code
    /// for a coded option.
    pub fn take_gossip_selects(&mut self) -> Vec<(u32, Option<String>)> {
        drain_field!(self, gossip_selects)
    }

    /// Whether `CloseGossip` was called since the last drain; the close sends no packet.
    pub fn take_gossip_close(&mut self) -> bool {
        drain_field!(self, gossip_close)
    }

    /// Drain the queued 1-based whole-menu quest-row positions.
    pub fn take_gossip_quest_selects(&mut self) -> Vec<u32> {
        drain_field!(self, gossip_quest_selects)
    }
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

    g.set(
        "GetGossipText",
        lua.create_function(|lua, ()| {
            let text = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                model.gossip.as_ref().map(|m| m.greeting.clone())
            };
            match text {
                Some(t) => Ok(Value::String(lua.create_string(&t)?)),
                None => Ok(Value::Nil),
            }
        })?,
    )?;

    g.set(
        "GetGossipOptions",
        lua.create_function(|lua, ()| {
            // Copy out under one short borrow; build the Lua values with none held.
            let pairs: Vec<(String, String)> = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                model.gossip.as_ref().map_or_else(Vec::new, |m| {
                    m.options
                        .iter()
                        .map(|o| (o.label.clone(), o.icon_type.clone()))
                        .collect()
                })
            };
            let mut out: Vec<Value> = Vec::with_capacity(pairs.len() * 2);
            for (label, icon) in pairs {
                out.push(Value::String(lua.create_string(&label)?));
                out.push(Value::String(lua.create_string(&icon)?));
            }
            Ok(MultiValue::from_vec(out))
        })?,
    )?;

    // The three counts (`0x58a920`, `0x58a960`, `0x58a9a0`); 0 with no menu.
    fn install_count(lua: &Lua, name: &str, count: fn(&GossipMenu) -> usize) -> mlua::Result<()> {
        lua.globals().set(
            name,
            lua.create_function(move |lua, ()| {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                Ok(model.gossip.as_ref().map_or(0, count) as i64)
            })?,
        )
    }
    install_count(lua, "GetNumGossipOptions", |m| m.options.len())?;
    install_count(lua, "GetNumGossipAvailableQuests", |m| {
        m.quests.iter().filter(|q| !q.active).count()
    })?;
    install_count(lua, "GetNumGossipActiveQuests", |m| {
        m.quests.iter().filter(|q| q.active).count()
    })?;

    // ForceGossip() (`0x58ab50`): a Lua boolean, false with no menu.
    g.set(
        "ForceGossip",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(model.gossip.as_ref().is_some_and(|m| m.force_gossip))
        })?,
    )?;

    // SelectGossipOption(index [, code [, confirmed]]) (`0x58b590` → `0x58af10`).
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

    g.set(
        "CloseGossip",
        lua.create_function(|lua, ()| {
            let mut model = crate::script::model_mut_of(lua);
            model.gossip_close = true;
            Ok(())
        })?,
    )?;

    // ══ The two quest lists ══
    // The reference splits the quest rows by wire icon, 3 and 4 active and the rest available
    // (`0x58a5d0`, `0x58a6c0`). An available row is `(title, level, isTrivial, isDaily,
    // isRepeatable)` (`0x58b3a0`), an active one `(title, level, isTrivial, isComplete)`
    // (`0x58b490`); the flags are 1 or nil.
    fn quest_rows(lua: &Lua, active: bool) -> mlua::Result<MultiValue> {
        let rows: Vec<(String, i32, [bool; 3])> = {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let player_level = model.player_req.level;
            model.gossip.as_ref().map_or_else(Vec::new, |m| {
                m.quests
                    .iter()
                    .filter(|q| q.active == active)
                    .map(|q| {
                        let trivial = quest_is_trivial(q.level, player_level);
                        let flags = if active {
                            let complete =
                                super::quest_log::quest_complete_at(&model.quest_log, q.quest_id);
                            [trivial, complete, false]
                        } else {
                            [trivial, q.daily, q.repeatable]
                        };
                        (q.title.clone(), q.level, flags)
                    })
                    .collect()
            })
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

    // A select's index is 1-based within its own list, as `GossipFrame.lua` numbers the buttons;
    // it maps back to the whole-menu position the app's queue holds.
    fn select_quest(lua: &Lua, active: bool, index: i32) {
        let whole = {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let nth = usize::try_from(index.wrapping_sub(1)).ok();
            model.gossip.as_ref().zip(nth).and_then(|(m, nth)| {
                m.quests
                    .iter()
                    .enumerate()
                    .filter(|(_, q)| q.active == active)
                    .nth(nth)
                    .map(|(n, _)| n as u32 + 1)
            })
        };
        if let Some(n) = whole {
            crate::script::model_mut_of(lua)
                .gossip_quest_selects
                .push(n);
        }
    }

    g.set(
        "SelectGossipAvailableQuest",
        lua.create_function(|lua, i: Value| {
            let i = number_arg(lua, i, "Usage: SelectGossipAvailableQuest(index)")?;
            select_quest(lua, false, i);
            Ok(())
        })?,
    )?;
    g.set(
        "SelectGossipActiveQuest",
        lua.create_function(|lua, i: Value| {
            let i = number_arg(lua, i, "Usage: SelectGossipActiveQuest(index)")?;
            select_quest(lua, true, i);
            Ok(())
        })?,
    )?;

    Ok(())
}

/// The select sender's checks, in its order (`0x58af10`): nothing while the greeting is pending
/// or for an index outside the menu; an unconfirmed box text asks (`0x58af74`), a coded option
/// with no code asks for one (`0x58afa5`), and box money past the purse refuses (`0x58afcd`).
/// A sent select empties the menu's options and quests (`0x58a7b0`) and fires
/// `GOSSIP_CONFIRM_CANCEL` (`0x58b051`); the greeting stays.
fn select_option(lua: &Lua, index: i32, code: Option<String>, confirmed: bool) {
    let mut model = crate::script::model_mut_of(lua);
    if model.gossip_pending {
        return;
    }
    let Some(slot) = usize::try_from(index.wrapping_sub(1)).ok() else {
        return;
    };
    let Some(opt) = model.gossip.as_ref().and_then(|m| m.options.get(slot)) else {
        return;
    };
    if let (Some(text), false) = (&opt.box_text, confirmed) {
        let args = vec![
            ScriptValue::Int(i64::from(index)),
            ScriptValue::Str(text.clone()),
            ScriptValue::Int(i64::from(opt.box_money)),
        ];
        model
            .pending_events
            .push(("GOSSIP_CONFIRM".to_string(), args));
        return;
    }
    let code = code.filter(|c| !c.is_empty());
    if opt.coded && code.is_none() {
        model.pending_events.push((
            "GOSSIP_ENTER_CODE".to_string(),
            vec![ScriptValue::Int(i64::from(index))],
        ));
        return;
    }
    if u64::from(opt.box_money) > model.money {
        model.ui_errors.push("ERR_NOT_ENOUGH_MONEY");
        return;
    }
    let code = if opt.coded { code } else { None };
    model.gossip_selects.push((index as u32, code));
    if let Some(menu) = model.gossip.as_mut() {
        menu.options.clear();
        menu.quests.clear();
    }
    model
        .pending_events
        .push(("GOSSIP_CONFIRM_CANCEL".to_string(), Vec::new()));
}

#[cfg(test)]
mod tests {
    use super::{quest_is_trivial, GossipMenu, GossipOptionView, GossipQuestRow};
    use crate::script::{ScriptValue, UiScript};

    fn option(label: &str, icon: &str) -> GossipOptionView {
        GossipOptionView {
            label: label.into(),
            icon_type: icon.into(),
            ..Default::default()
        }
    }

    fn menu() -> GossipMenu {
        GossipMenu {
            greeting: "Greetings, traveler.".into(),
            quests: Vec::new(),
            options: vec![
                option("Let me browse your goods.", "vendor"),
                option("I would like to sign the petition.", "gossip"),
            ],
            force_gossip: false,
        }
    }

    fn fired(s: &UiScript) -> Vec<(String, Vec<ScriptValue>)> {
        std::mem::take(&mut s.model_mut().pending_events)
    }

    #[test]
    fn gossip_snapshot_reads_and_selects_queue() {
        let mut s = UiScript::new().unwrap();
        assert!(s.eval::<bool>("return GetGossipText() == nil").unwrap());
        assert_eq!(s.arity("GetGossipOptions()").unwrap(), 0);
        assert_eq!(s.eval::<i64>("return GetNumGossipOptions()").unwrap(), 0);

        s.set_gossip(Some(menu()));
        assert_eq!(
            s.eval::<String>("return GetGossipText()").unwrap(),
            "Greetings, traveler."
        );
        assert_eq!(s.eval::<i64>("return GetNumGossipOptions()").unwrap(), 2);
        let (l1, t1, l2, t2) = s
            .eval::<(String, String, String, String)>(
                "local a,b,c,d = GetGossipOptions()\n\
                 return a, b, c, d",
            )
            .unwrap();
        assert_eq!(s.arity("GetGossipOptions()").unwrap(), 4);
        assert_eq!(
            (l1.as_str(), t1.as_str()),
            ("Let me browse your goods.", "vendor")
        );
        assert_eq!(l2, "I would like to sign the petition.");
        assert_eq!(t2, "gossip");

        // A code on an uncoded option is dropped; the select empties the menu.
        s.run("SelectGossipOption(2, 'unused-code')").unwrap();
        assert_eq!(s.take_gossip_selects(), vec![(2, None)]);
        assert_eq!(s.eval::<i64>("return GetNumGossipOptions()").unwrap(), 0);
        assert_eq!(
            s.eval::<String>("return GetGossipText()").unwrap(),
            "Greetings, traveler.",
            "the greeting stays"
        );
        let names: Vec<String> = fired(&s).into_iter().map(|(e, _)| e).collect();
        assert_eq!(names, vec!["GOSSIP_CONFIRM_CANCEL"]);
        s.run("SelectGossipOption(1)").unwrap();
        assert!(s.take_gossip_selects().is_empty(), "a sent menu is spent");

        assert!(!s.take_gossip_close());
        s.run("CloseGossip()").unwrap();
        assert!(s.take_gossip_close());
        assert!(!s.take_gossip_close(), "drained");
    }

    #[test]
    fn select_asks_for_confirmation_then_a_code_then_the_money() {
        let mut s = UiScript::new().unwrap();
        let mut m = menu();
        m.options = vec![GossipOptionView {
            label: "Open the vault.".into(),
            icon_type: "gossip".into(),
            coded: true,
            box_text: Some("Are you sure?".into()),
            box_money: 500,
        }];
        s.set_gossip(Some(m.clone()));

        s.run("SelectGossipOption(1)").unwrap();
        assert!(s.take_gossip_selects().is_empty());
        assert_eq!(
            fired(&s),
            vec![(
                "GOSSIP_CONFIRM".to_string(),
                vec![
                    ScriptValue::Int(1),
                    ScriptValue::Str("Are you sure?".into()),
                    ScriptValue::Int(500)
                ]
            )]
        );

        // The stock popup's accept passes "" and true: the code is asked next.
        s.run("SelectGossipOption(1, '', true)").unwrap();
        assert!(s.take_gossip_selects().is_empty());
        assert_eq!(
            fired(&s),
            vec![("GOSSIP_ENTER_CODE".to_string(), vec![ScriptValue::Int(1)])]
        );

        // Too poor: refused with the error and no select.
        s.model_mut().money = 499;
        s.run("SelectGossipOption(1, '1234', true)").unwrap();
        assert!(s.take_gossip_selects().is_empty());
        assert_eq!(s.take_ui_errors(), vec!["ERR_NOT_ENOUGH_MONEY"]);

        s.model_mut().money = 500;
        s.run("SelectGossipOption(1, '1234', 1)").unwrap();
        assert_eq!(s.take_gossip_selects(), vec![(1, Some("1234".into()))]);
    }

    #[test]
    fn select_is_refused_while_the_greeting_is_pending_or_out_of_range() {
        let mut s = UiScript::new().unwrap();
        s.set_gossip(Some(menu()));
        s.set_gossip_pending(true);
        s.run("SelectGossipOption(1)").unwrap();
        assert!(s.take_gossip_selects().is_empty());
        s.set_gossip_pending(false);
        s.run("SelectGossipOption(0) SelectGossipOption(3)")
            .unwrap();
        assert!(s.take_gossip_selects().is_empty());
        assert!(fired(&s).is_empty());
        assert!(s.run("SelectGossipOption()").is_err(), "Usage: raises");
    }

    #[test]
    fn the_two_gossip_quest_lists_split_by_active_and_select_within_themselves() {
        let mut s = UiScript::new().unwrap();
        // No menu: `arg.n == 0`, not a nil.
        assert_eq!(s.arity("GetGossipAvailableQuests()").unwrap(), 0);
        assert_eq!(s.arity("GetGossipActiveQuests()").unwrap(), 0);

        let mut m = menu();
        m.quests = vec![
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
        s.set_gossip(Some(m));
        s.model_mut().player_req.level = 10;
        // Quest 60 is complete in the log, which `isComplete` reads (`0x58bcb0`).
        s.set_quest_log(crate::script::QuestLogState {
            entries: vec![crate::script::QuestLogEntryView {
                quest_id: 60,
                title: "Report Back".into(),
                complete: 1,
                ..Default::default()
            }],
            num_quests: 1,
            ..Default::default()
        });

        assert_eq!(
            s.eval::<i64>("return GetNumGossipAvailableQuests()")
                .unwrap(),
            2
        );
        assert_eq!(
            s.eval::<i64>("return GetNumGossipActiveQuests()").unwrap(),
            1
        );
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
        // Four per active row; the active row comes first in the menu but splits by icon.
        assert_eq!(s.arity("GetGossipActiveQuests()").unwrap(), 4);
        assert!(s
            .eval::<bool>(
                "local t,l,triv,done = GetGossipActiveQuests()\n\
                 return t == 'Report to Goldshire' and l == 5 and triv == nil and done == 1"
            )
            .unwrap());

        // Available #2 is the menu's third row.
        s.run("SelectGossipAvailableQuest(2)").unwrap();
        assert_eq!(s.take_gossip_quest_selects(), vec![3]);
        s.run("SelectGossipActiveQuest(1)").unwrap();
        assert_eq!(s.take_gossip_quest_selects(), vec![1]);
        s.run("SelectGossipAvailableQuest(9) SelectGossipActiveQuest(0)")
            .unwrap();
        assert!(s.take_gossip_quest_selects().is_empty(), "drained");
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
    fn force_gossip_reads_the_menu_flag() {
        let mut s = UiScript::new().unwrap();
        assert!(!s.eval::<bool>("return ForceGossip()").unwrap());
        let mut m = menu();
        m.force_gossip = true;
        s.set_gossip(Some(m));
        assert!(s.eval::<bool>("return ForceGossip()").unwrap());
    }

    #[test]
    fn clearing_the_menu_empties_it() {
        let mut s = UiScript::new().unwrap();
        s.set_gossip(Some(menu()));
        s.set_gossip(None);
        assert!(s.eval::<bool>("return GetGossipText() == nil").unwrap());
        assert_eq!(s.arity("GetGossipOptions()").unwrap(), 0);
    }
}
