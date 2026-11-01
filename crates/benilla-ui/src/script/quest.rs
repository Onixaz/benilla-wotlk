//! The questgiver bindings: the app pushes the open panel ([`UiScript::set_quest`]), and the panel
//! verbs queue selects and button intents it drains. One [`QuestState`] serves the flat getters
//! `QuestFrame.lua` reads on each panel; the app fires `QUEST_GREETING`, `QUEST_DETAIL`,
//! `QUEST_PROGRESS` or `QUEST_COMPLETE` for the live one.

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::flag;
use super::gossip::quest_is_trivial;
use super::GossipQuestRow;

/// Which questgiver panel a [`QuestState`] is for, set from the wire packet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuestPanel {
    /// `SMSG_QUESTGIVER_QUEST_LIST`: the greeting, with the active and available lists.
    Greeting,
    /// `SMSG_QUESTGIVER_QUEST_DETAILS`: the accept panel.
    Detail,
    /// `SMSG_QUESTGIVER_REQUEST_ITEMS`: the turn-in progress panel.
    Progress,
    /// `SMSG_QUESTGIVER_OFFER_REWARD`: the reward panel.
    Reward,
}

/// One choice, reward or required item row; its 1-based index is its place in its list.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct QuestItemView {
    /// `None` while the item template is in flight: `GetQuestItemInfo` answers an empty name, the
    /// log getters their miss.
    pub name: Option<String>,
    pub texture: Option<String>,
    pub count: u32,
    /// 0 poor to 5 legendary; 1 until the template lands.
    pub quality: u32,
    /// The item id the quest tooltips render by; 0 until the wire row resolves.
    pub item_id: u32,
    /// Whether the player can use the item, `isUsable` as 1 or nil; stock tints the row red when
    /// not (`QuestFrame.lua:393`). The app sends `true` and the getters run the can-use test.
    pub usable: bool,
    /// The escaped item link `GetQuestItemLink` and `GetQuestLogItemLink` serve; `None` until the
    /// template lands, since it embeds the name and the quality.
    pub link: Option<String>,
}

/// The reward spell, `rewSpell` on the giver packets and `SMSG_QUEST_QUERY_RESPONSE`, as
/// `GetRewardSpell` and `GetQuestLogRewardSpell` answer it; stock counts it as one more reward slot
/// (`QuestInfo.lua:281`). `tradeskill` is the third return, `isTradeskillSpell`, and `learned` the
/// fourth, `isSpellLearned`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct QuestRewardSpell {
    pub spell_id: u32,
    pub name: Option<String>,
    pub texture: Option<String>,
    pub tradeskill: bool,
    /// The spell cast at turn-in (`rewSpellCast`) teaches one: an effect of 36 or 57
    /// (`0x58d670`, `0x5e4d10`).
    pub learned: bool,
}

/// The rewards past items and money that 3.3.5a's quest frame and log both show, resolved by the
/// app: the quest-frame verbs read the panel's (`GetRewardXP`, …), the log's the selection's.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct QuestRewardExtras {
    pub xp: u32,
    pub honor: u32,
    pub talents: u32,
    pub arena_points: u32,
    /// The title reward's text for the player's gender, `None` for no title.
    pub title: Option<String>,
    /// `(faction id, amount × 100)`, the reward and its spillover in the reference's order, at
    /// most ten: the list `GetQuestLogRewardFactionInfo` reads (`0xc01818`, `0xc01840`).
    pub factions: Vec<(u32, i32)>,
}

/// One open questgiver panel, with every field the four panels read; pushed whole by the app.
#[derive(Clone, Debug, PartialEq)]
pub struct QuestState {
    pub panel: QuestPanel,
    // Greeting panel.
    pub greeting: String,
    /// The greeting's rows in wire order, split active and available by icon as gossip's are
    /// (`0x6d039a`); each list keeps its own order and 1-based indexes.
    pub greeting_quests: Vec<GossipQuestRow>,
    // Detail, progress and reward panels.
    pub title: String,
    /// The text of whichever panel is live: quest, progress or reward.
    pub body: String,
    pub objectives: String,
    /// Rewards the player picks one of.
    pub choices: Vec<QuestItemView>,
    /// Rewards all granted.
    pub rewards: Vec<QuestItemView>,
    /// Items the progress panel asks for.
    pub required: Vec<QuestItemView>,
    /// In copper.
    pub reward_money: u32,
    /// In copper.
    pub required_money: u32,
    pub completable: bool,
    pub reward_spell: Option<QuestRewardSpell>,
    /// `GetQuestBackgroundMaterial`, which the app fills from an item or GameObject source, never
    /// from the wire; nil reads as "Parchment" (`QuestFrame.lua:598-604`).
    pub background_material: Option<String>,
    /// The detail and reward panels' extra rewards.
    pub extras: QuestRewardExtras,
    /// `GetSuggestedGroupNum` (`0x58c3b0`, the panel's suggested players).
    pub suggested_group: u32,
    /// `QuestFlagsPVP` (`0x58c3e0`): the quest's `0x2000` flag, never on a PvP realm.
    pub flags_pvp: bool,
    /// `QuestGetAutoAccept` (`0x58c430`): the quest's `0x80000` flag.
    pub auto_accept: bool,
}

impl Default for QuestState {
    fn default() -> Self {
        QuestState {
            panel: QuestPanel::Detail,
            greeting: String::new(),
            greeting_quests: Vec::new(),
            title: String::new(),
            body: String::new(),
            objectives: String::new(),
            choices: Vec::new(),
            rewards: Vec::new(),
            required: Vec::new(),
            reward_money: 0,
            required_money: 0,
            completable: false,
            reward_spell: None,
            background_material: None,
            extras: QuestRewardExtras::default(),
            suggested_group: 0,
            flags_pvp: false,
            auto_accept: false,
        }
    }
}

/// A greeting row picked by `SelectActiveQuest` or `SelectAvailableQuest`, 1-based in its list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QuestSelect {
    pub active: bool,
    pub index: u32,
}

/// A questgiver button intent; the app sends the matching message for its NPC and quest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuestAction {
    /// Detail panel Accept → `CMSG_QUESTGIVER_ACCEPT_QUEST`.
    Accept,
    /// Progress panel Continue → `CMSG_QUESTGIVER_REQUEST_REWARD`.
    Continue,
    /// Reward panel Complete → `CMSG_QUESTGIVER_CHOOSE_REWARD` with the 0-based choice.
    Reward(u32),
    /// `DeclineQuest()`: the detail panel's Decline and the progress and reward panels' Cancel. Its
    /// binding re-opens the giver: `CMSG_GOSSIP_HELLO` for a gossip unit, `CMSG_QUESTGIVER_HELLO`
    /// for another unit, the teardown for an item or a player, a GameObject's interact call. The
    /// app owns this.
    Decline,
    /// `CloseQuest()`, from `QuestFrame_OnHide`: ESC, the close button, the greeting's Goodbye, a
    /// panel eviction. Its binding calls only the teardown, whose one send is
    /// `MSG_QUEST_PUSH_RESULT` DECLINE for a player source, so closing an NPC's window sends
    /// nothing. It stays apart from `Decline`: the re-open is `DeclineQuest`'s alone.
    Close,
}

impl super::UiScript {
    /// Push (or clear, with `None`) the open questgiver panel snapshot. A detail or reward panel
    /// refills the reward-faction list, as its publisher does (`0x58dd50` → `0x58d050`).
    pub fn set_quest(&mut self, state: Option<QuestState>) {
        let mut model = self.model_mut();
        if let Some(q) = &state {
            if matches!(q.panel, QuestPanel::Detail | QuestPanel::Reward) {
                model.quest_reward_factions = q.extras.factions.clone();
            }
        }
        model.quest = state;
    }

    /// Drain the greeting-panel row selects queued since the last call.
    pub fn take_quest_selects(&mut self) -> Vec<QuestSelect> {
        drain_field!(self, quest_selects)
    }

    /// Drain the button intents queued since the last call.
    pub fn take_quest_actions(&mut self) -> Vec<QuestAction> {
        drain_field!(self, quest_actions)
    }
}

/// `GetRewardSpell`'s four returns: `texture, name, isTradeskillSpell, isSpellLearned`, the last
/// two 1 or nil.
pub(super) fn reward_spell_returns(
    lua: &Lua,
    spell: Option<QuestRewardSpell>,
) -> mlua::Result<MultiValue> {
    let Some(sp) = spell else {
        return Ok(MultiValue::from_vec(vec![Value::Nil; 4]));
    };
    let texture = match &sp.texture {
        Some(t) => Value::String(lua.create_string(t)?),
        None => Value::Nil,
    };
    let name = match &sp.name {
        Some(n) => Value::String(lua.create_string(n)?),
        None => Value::Nil,
    };
    Ok(MultiValue::from_vec(vec![
        texture,
        name,
        flag(sp.tradeskill),
        flag(sp.learned),
    ]))
}

/// A reward spell's link (the tooltip's spell link, `0x61e4b0`), nil for none or for an id with no
/// `Spell.dbc` row.
pub(super) fn reward_spell_link(
    lua: &Lua,
    spell: Option<&QuestRewardSpell>,
) -> mlua::Result<Value> {
    match spell.and_then(|sp| Some((sp.spell_id, sp.name.as_deref()?))) {
        Some((id, name)) => Ok(Value::String(
            lua.create_string(format!("|cff71d5ff|Hspell:{id}|h[{name}]|h|r"))?,
        )),
        None => Ok(Value::Nil),
    }
}

/// The icon an empty offer row answers: display 0 has no `ItemDisplayInfo` row, so the icon lookup
/// falls back to the question mark under the icon folder.
const EMPTY_ROW_ICON: &str = "Interface\\Icons\\INV_Misc_QuestionMark";

/// The greeting rows of one list, active or available, in their 1-based order.
fn greeting_rows(state: &QuestState, active: bool) -> impl Iterator<Item = &GossipQuestRow> {
    state
        .greeting_quests
        .iter()
        .filter(move |r| r.active == active)
}

/// The list `GetQuestItemInfo(type, index)` reads for `type`.
pub(super) fn item_vec<'a>(state: &'a QuestState, kind: &str) -> Option<&'a Vec<QuestItemView>> {
    match kind {
        "choice" => Some(&state.choices),
        "reward" => Some(&state.rewards),
        "required" => Some(&state.required),
        _ => None,
    }
}

/// Register the questgiver globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // ── Text getters ──
    fn install_text(lua: &Lua, name: &str, pick: fn(&QuestState) -> String) -> mlua::Result<()> {
        lua.globals().set(
            name,
            lua.create_function(move |lua, ()| {
                let text = {
                    let model = lua
                        .app_data_ref::<crate::script::ModelCell>()
                        .expect("model app_data");
                    model.quest.as_ref().map(pick).unwrap_or_default()
                };
                Ok(Value::String(lua.create_string(&text)?))
            })?,
        )
    }
    install_text(lua, "GetGreetingText", |q| q.greeting.clone())?;
    install_text(lua, "GetTitleText", |q| q.title.clone())?;
    install_text(lua, "GetQuestText", |q| q.body.clone())?;
    install_text(lua, "GetProgressText", |q| q.body.clone())?;
    install_text(lua, "GetRewardText", |q| q.body.clone())?;
    install_text(lua, "GetObjectiveText", |q| q.objectives.clone())?;

    // ── Count and money getters ──
    fn install_count(lua: &Lua, name: &str, pick: fn(&QuestState) -> i64) -> mlua::Result<()> {
        lua.globals().set(
            name,
            lua.create_function(move |lua, ()| {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                Ok(model.quest.as_ref().map(pick).unwrap_or(0))
            })?,
        )
    }
    install_count(lua, "GetNumActiveQuests", |q| {
        greeting_rows(q, true).count() as i64
    })?;
    install_count(lua, "GetNumAvailableQuests", |q| {
        greeting_rows(q, false).count() as i64
    })?;
    install_count(lua, "GetNumQuestChoices", |q| q.choices.len() as i64)?;
    install_count(lua, "GetNumQuestRewards", |q| q.rewards.len() as i64)?;
    install_count(lua, "GetNumQuestItems", |q| q.required.len() as i64)?;
    install_count(lua, "GetRewardMoney", |q| i64::from(q.reward_money))?;
    install_count(lua, "GetQuestMoneyToGet", |q| i64::from(q.required_money))?;
    // The panel's extra rewards (`0x58c0e0`, `0x58c110`, `0x58c140`, `0x58c4a0`, `0x58c3b0`).
    install_count(lua, "GetRewardXP", |q| i64::from(q.extras.xp))?;
    install_count(lua, "GetRewardHonor", |q| i64::from(q.extras.honor))?;
    install_count(lua, "GetRewardTalents", |q| i64::from(q.extras.talents))?;
    install_count(lua, "GetRewardArenaPoints", |q| {
        i64::from(q.extras.arena_points)
    })?;
    install_count(lua, "GetSuggestedGroupNum", |q| {
        i64::from(q.suggested_group)
    })?;
    fn install_flag(lua: &Lua, name: &str, pick: fn(&QuestState) -> bool) -> mlua::Result<()> {
        lua.globals().set(
            name,
            lua.create_function(move |lua, ()| {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                Ok(flag(model.quest.as_ref().is_some_and(pick)))
            })?,
        )
    }
    install_flag(lua, "QuestFlagsPVP", |q| q.flags_pvp)?;
    install_flag(lua, "QuestGetAutoAccept", |q| q.auto_accept)?;
    // `GetRewardTitle` (`0x58d810`): the title's text, nil for none.
    g.set(
        "GetRewardTitle",
        lua.create_function(|lua, ()| {
            let title = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                model.quest.as_ref().and_then(|q| q.extras.title.clone())
            };
            match title {
                Some(t) => Ok(Value::String(lua.create_string(&t)?)),
                None => Ok(Value::Nil),
            }
        })?,
    )?;

    g.set(
        "IsQuestCompletable",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(flag(model.quest.as_ref().is_some_and(|q| q.completable)))
        })?,
    )?;

    // ── The greeting's rows ──
    // Each takes the 1-based row, raising its usage line on a non-number, and reads a row past the
    // list as level 0: a nil title, never trivial, no flag. The level getters' usage lines carry
    // the reference's doubled `GetGet`.
    fn install_row<F>(
        lua: &Lua,
        name: &str,
        usage: &'static str,
        active: bool,
        answer: F,
    ) -> mlua::Result<()>
    where
        F: Fn(&Lua, Option<&GossipQuestRow>, &crate::script::Model) -> mlua::Result<MultiValue>
            + 'static,
    {
        lua.globals().set(
            name,
            lua.create_function(move |lua, index: Value| {
                // `(uint32)index - 1`, so 0 and below wrap past every row.
                let i = super::binding_abi::number_arg(lua, index, usage)? as u32;
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                let row = i.checked_sub(1).and_then(|i| {
                    model
                        .quest
                        .as_ref()
                        .and_then(|q| greeting_rows(q, active).nth(i as usize))
                });
                answer(lua, row, &model)
            })?,
        )
    }
    fn one(v: Value) -> mlua::Result<MultiValue> {
        Ok(MultiValue::from_vec(vec![v]))
    }
    fn title(lua: &Lua, row: Option<&GossipQuestRow>) -> mlua::Result<Value> {
        match row {
            Some(r) => Ok(Value::String(lua.create_string(&r.title)?)),
            None => Ok(Value::Nil),
        }
    }
    fn level(row: Option<&GossipQuestRow>) -> i32 {
        row.map_or(0, |r| r.level)
    }
    fn trivial(row: Option<&GossipQuestRow>, model: &crate::script::Model) -> bool {
        quest_is_trivial(level(row), model.player_req.level)
    }
    // `GetActiveTitle` (`0x58bed0`): the title, then `isComplete` as a boolean (the completion
    // test, `0x58bcb0`).
    install_row(
        lua,
        "GetActiveTitle",
        "Usage: GetActiveTitle(index)",
        true,
        |lua, row, model| {
            let complete = row
                .is_some_and(|r| super::quest_log::quest_complete_at(&model.quest_log, r.quest_id));
            Ok(MultiValue::from_vec(vec![
                title(lua, row)?,
                Value::Boolean(complete),
            ]))
        },
    )?;
    // `GetAvailableTitle` (`0x58be30`).
    install_row(
        lua,
        "GetAvailableTitle",
        "Usage: GetAvailableTitle(index)",
        false,
        |lua, row, _| one(title(lua, row)?),
    )?;
    // `GetActiveLevel` (`0x58c010`), `GetAvailableLevel` (`0x58bf70`).
    install_row(
        lua,
        "GetActiveLevel",
        "Usage: GetGetActiveLevel(index)",
        true,
        |_, row, _| one(Value::Integer(i64::from(level(row)))),
    )?;
    install_row(
        lua,
        "GetAvailableLevel",
        "Usage: GetGetAvailableLevel(index)",
        false,
        |_, row, _| one(Value::Integer(i64::from(level(row)))),
    )?;
    // `IsActiveQuestTrivial` (`0x58d420`), `IsAvailableQuestTrivial` (`0x58d370`): 1 or nil.
    install_row(
        lua,
        "IsActiveQuestTrivial",
        "Usage: IsActiveQuestTrivial(index)",
        true,
        |_, row, model| one(flag(trivial(row, model))),
    )?;
    install_row(
        lua,
        "IsAvailableQuestTrivial",
        "Usage: IsAvailableQuestTrivial(index)",
        false,
        |_, row, model| one(flag(trivial(row, model))),
    )?;
    // `GetAvailableQuestInfo` (`0x58dbc0`): isTrivial, isDaily (the `0x1000` flag) and
    // isRepeatable, each 1 or nil.
    install_row(
        lua,
        "GetAvailableQuestInfo",
        "Usage: GetAvailableQuestInfo(index)",
        false,
        |_, row, model| {
            Ok(MultiValue::from_vec(vec![
                flag(trivial(row, model)),
                flag(row.is_some_and(|r| r.daily)),
                flag(row.is_some_and(|r| r.repeatable)),
            ]))
        },
    )?;

    // GetQuestItemInfo(type, index) → name, texture, numItems, quality, isUsable: five values,
    // `isUsable` 1 or nil. A type other than `"reward"`, `"choice"` or `"required"` (any case) or
    // an index outside 1..7 raises, as does a non-string type or non-number index. An index inside
    // that range with no item answers the zeroed row the offer publisher leaves: no name, the
    // question-mark icon of display 0, count 0, quality 0 and usable. A template in flight answers
    // an empty name.
    g.set(
        "GetQuestItemInfo",
        lua.create_function(|lua, (kind, index): (Value, Value)| {
            let invalid =
                || mlua::Error::runtime(r#"Invalid quest item in GetQuestItemInfo("type", index)"#);
            let kind = match &kind {
                Value::String(_) | Value::Integer(_) | Value::Number(_) => lua
                    .coerce_string(kind.clone())?
                    .map(|s| s.to_string_lossy().to_ascii_lowercase()),
                _ => None,
            }
            .ok_or_else(invalid)?;
            let index = lua.coerce_number(index)?.ok_or_else(invalid)?;
            // `_ftol` then `dec`, compared unsigned against 6.
            let row = index.trunc() as i64 - 1;
            if !(0..=6).contains(&row) {
                return Err(invalid());
            }
            let item = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                let empty = Vec::new();
                let list = match model.quest.as_ref() {
                    Some(q) => item_vec(q, &kind),
                    None => ["reward", "choice", "required"]
                        .contains(&kind.as_str())
                        .then_some(&empty),
                };
                let Some(list) = list else {
                    return Err(invalid());
                };
                list.get(row as usize).cloned().map(|it| {
                    let verdict = super::item_stats::quest_item_quality_usable(
                        &model, it.item_id, it.quality, it.usable,
                    );
                    (it, verdict)
                })
            };
            let (name, texture, count, quality, usable) = match item {
                Some((it, (quality, usable))) => (
                    it.name.unwrap_or_default(),
                    it.texture,
                    it.count,
                    quality,
                    usable,
                ),
                None => (String::new(), Some(EMPTY_ROW_ICON.to_string()), 0, 0, true),
            };
            Ok(MultiValue::from_vec(vec![
                Value::String(lua.create_string(&name)?),
                match texture {
                    Some(t) => Value::String(lua.create_string(&t)?),
                    None => Value::Nil,
                },
                Value::Integer(i64::from(count)),
                Value::Integer(quality),
                super::binding_abi::flag(usable),
            ]))
        })?,
    )?;

    // GetQuestItemLink(type, index), for a row's ctrl and shift clicks (`QuestFrame.lua:118`).
    g.set(
        "GetQuestItemLink",
        lua.create_function(|lua, (kind, index): (String, usize)| {
            let link = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                model.quest.as_ref().and_then(|q| {
                    item_vec(q, &kind)
                        .and_then(|v| index.checked_sub(1).and_then(|n| v.get(n)))
                        .and_then(|it| it.link.clone())
                })
            };
            match link {
                Some(l) => Ok(Value::String(lua.create_string(&l)?)),
                None => Ok(Value::Nil),
            }
        })?,
    )?;

    // GetRewardSpell() (`0x58d670`): four nils without a spell.
    g.set(
        "GetRewardSpell",
        lua.create_function(|lua, ()| {
            let spell = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                model.quest.as_ref().and_then(|q| q.reward_spell.clone())
            };
            reward_spell_returns(lua, spell)
        })?,
    )?;
    // `GetQuestSpellLink` (`0x58dab0`).
    g.set(
        "GetQuestSpellLink",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            reward_spell_link(
                lua,
                model.quest.as_ref().and_then(|q| q.reward_spell.as_ref()),
            )
        })?,
    )?;

    // ── Intents ──
    fn install_select(lua: &Lua, name: &str, active: bool) -> mlua::Result<()> {
        lua.globals().set(
            name,
            lua.create_function(move |lua, i: u32| {
                let mut model = crate::script::model_mut_of(lua);
                model
                    .quest_selects
                    .push(super::QuestSelect { active, index: i });
                Ok(())
            })?,
        )
    }
    install_select(lua, "SelectActiveQuest", true)?;
    install_select(lua, "SelectAvailableQuest", false)?;

    fn install_action(lua: &Lua, name: &str, action: super::QuestAction) -> mlua::Result<()> {
        lua.globals().set(
            name,
            lua.create_function(move |lua, ()| {
                let mut model = crate::script::model_mut_of(lua);
                model.quest_actions.push(action);
                Ok(())
            })?,
        )
    }
    install_action(lua, "AcceptQuest", super::QuestAction::Accept)?;
    install_action(lua, "DeclineQuest", super::QuestAction::Decline)?;
    install_action(lua, "CloseQuest", super::QuestAction::Close)?;
    install_action(lua, "CompleteQuest", super::QuestAction::Continue)?;

    // GetQuestBackgroundMaterial() → nil or a material name.
    g.set(
        "GetQuestBackgroundMaterial",
        lua.create_function(|lua, ()| {
            let material = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                model
                    .quest
                    .as_ref()
                    .and_then(|q| q.background_material.clone())
            };
            match material {
                Some(m) => Ok(Value::String(lua.create_string(&m)?)),
                None => Ok(Value::Nil),
            }
        })?,
    )?;

    // QuestChooseRewardError(), no returns: Complete pressed with choices and none picked
    // (`QuestFrame.lua:98`). It shows game error `0x98`: the sound `igQuestFailed`, then
    // `ERR_QUEST_MUST_CHOOSE` through `UI_ERROR_MESSAGE`, synchronously.
    g.set(
        "QuestChooseRewardError",
        lua.create_function(|lua, ()| {
            {
                let mut model = crate::script::model_mut_of(lua);
                model
                    .sound_queue
                    .push(super::SoundRequest::KitName("igQuestFailed".to_string()));
            }
            let text: String = lua
                .globals()
                .get::<Option<String>>("ERR_QUEST_MUST_CHOOSE")?
                .unwrap_or_else(|| "You must choose a reward.".to_string());
            super::tick::fire_event_into(
                lua,
                "UI_ERROR_MESSAGE",
                vec![super::ScriptValue::Str(text)],
            );
            Ok(())
        })?,
    )?;

    // ConfirmAcceptQuest(): the escort confirm's Yes (`StaticPopup.lua:731-733`). It takes no quest
    // id, as the client answers the confirm it holds, so calls are counted and the app adds the id.
    g.set(
        "ConfirmAcceptQuest",
        lua.create_function(|lua, ()| {
            let mut model = crate::script::model_mut_of(lua);
            model.quest_confirms += 1;
            Ok(())
        })?,
    )?;

    // GetQuestReward(choice): stock passes the 1-based row (`QuestFrame.lua:100`), and the wire
    // takes it less one, floored at 0, as in the reference; vmangos indexes `RewChoiceItemId` with
    // it.
    g.set(
        "GetQuestReward",
        lua.create_function(|lua, choice: Option<Value>| {
            // A missing or non-number choice reads -1 (`0x58d62b`); with choices on the reward
            // panel, one out of range raises (`0x58cff1`-`0x58cff9`, `0x58d65c`).
            let index = match choice {
                Some(v) => lua.coerce_number(v)?.map_or(-1, |n| n as i64 - 1),
                None => -1,
            };
            let mut model = crate::script::model_mut_of(lua);
            let choices = model
                .quest
                .as_ref()
                .filter(|q| q.panel == QuestPanel::Reward)
                .map_or(0, |q| q.choices.len() as i64);
            if choices > 0 && !(0..choices).contains(&index) {
                return Err(mlua::Error::RuntimeError(
                    "Invalid reward choice in GetQuestReward([choice])".into(),
                ));
            }
            model
                .quest_actions
                .push(super::QuestAction::Reward(index.max(0) as u32));
            Ok(())
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{GossipQuestRow, QuestAction, QuestItemView, QuestPanel, QuestSelect, QuestState};
    use crate::script::UiScript;

    fn detail() -> QuestState {
        QuestState {
            panel: QuestPanel::Detail,
            title: "A Threat Within".into(),
            body: "Kill the kobolds infesting the mine.".into(),
            objectives: "Slay 10 Kobold Vermin.".into(),
            rewards: vec![QuestItemView {
                item_id: 7278,
                name: Some("Brdle Leather Boots".into()),
                texture: Some("Interface\\Icons\\INV_Boots_01".into()),
                count: 1,
                quality: 2,
                usable: true,
                link: Some("|cff1eff00|Hitem:7278:0:0:0|h[Brdle Leather Boots]|h|r".into()),
            }],
            choices: vec![
                QuestItemView {
                    item_id: 0,
                    name: Some("Cudgel".into()),
                    texture: None,
                    count: 1,
                    quality: 1,
                    usable: true,
                    ..Default::default()
                },
                QuestItemView {
                    item_id: 0,
                    name: None, // template still in flight
                    texture: None,
                    count: 1,
                    quality: 1,
                    usable: true,
                    ..Default::default()
                },
            ],
            reward_money: 1234,
            ..Default::default()
        }
    }

    #[test]
    fn background_material_answers_the_pushed_material_or_nil() {
        let mut s = UiScript::new().unwrap();
        s.set_quest(Some(QuestState::default()));
        assert!(s
            .eval::<bool>("return GetQuestBackgroundMaterial() == nil")
            .unwrap());
        s.set_quest(Some(QuestState {
            background_material: Some("Stone".into()),
            ..QuestState::default()
        }));
        assert_eq!(
            s.eval::<String>("return GetQuestBackgroundMaterial()")
                .unwrap(),
            "Stone"
        );
    }

    #[test]
    fn choose_reward_error_fires_the_error_event_with_the_global_string() {
        let mut s = UiScript::new().unwrap();
        s.run(
            "ERR_QUEST_MUST_CHOOSE = 'Pick one.' \
             local f = CreateFrame('Frame') f:RegisterEvent('UI_ERROR_MESSAGE') \
             f:SetScript('OnEvent', function(self, event, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9) SEEN = arg1 end) \
             N = table.getn({QuestChooseRewardError()})",
        )
        .unwrap();
        assert_eq!(s.eval::<String>("return SEEN").unwrap(), "Pick one.");
        assert_eq!(s.eval::<i64>("return N").unwrap(), 0, "zero return values");
        assert_eq!(
            s.take_sounds(),
            vec![crate::script::SoundRequest::KitName("igQuestFailed".into())],
            "row 0x98's cue plays with the message"
        );
    }

    /// The reader clears `isUsable` when the can-use test fails (`0x58d32c`) and answers quality -1
    /// for a template with inventory type 0 (`0x58d29f`).
    #[test]
    fn a_reward_the_player_cannot_use_answers_unusable() {
        let mut s = UiScript::new().unwrap();
        s.set_quest(Some(detail()));
        s.set_player_req_state(crate::script::PlayerReqState {
            level: 10,
            class_id: 1, // warrior
            race_id: 1,
            ..Default::default()
        });
        let boots = crate::script::ItemTemplateView {
            quality: 2,
            inventory_type: 8,
            allowable_class: -1,
            allowable_race: -1,
            ..Default::default()
        };
        s.set_item_template(7278, boots.clone());
        let row = r#"local _, _, _, q, u = GetQuestItemInfo("reward", 1) return q, u == 1"#;
        assert_eq!(s.eval::<(i64, bool)>(row).unwrap(), (2, true));
        // Rogue-only: the warrior cannot use it.
        s.set_item_template(
            7278,
            crate::script::ItemTemplateView {
                allowable_class: 1 << 3,
                ..boots.clone()
            },
        );
        assert_eq!(s.eval::<(i64, bool)>(row).unwrap(), (2, false));
        // No inventory type: quality -1.
        s.set_item_template(
            7278,
            crate::script::ItemTemplateView {
                inventory_type: 0,
                ..boots
            },
        );
        assert_eq!(s.eval::<(i64, bool)>(row).unwrap(), (-1, true));
    }

    #[test]
    fn detail_panel_getters_read() {
        let mut s = UiScript::new().unwrap();
        // No window: text empty, counts zero.
        assert_eq!(s.eval::<String>("return GetTitleText()").unwrap(), "");
        assert_eq!(s.eval::<i64>("return GetNumQuestChoices()").unwrap(), 0);

        s.set_quest(Some(detail()));
        assert_eq!(
            s.eval::<String>("return GetTitleText()").unwrap(),
            "A Threat Within"
        );
        assert_eq!(
            s.eval::<String>("return GetObjectiveText()").unwrap(),
            "Slay 10 Kobold Vermin."
        );
        assert_eq!(s.eval::<i64>("return GetNumQuestChoices()").unwrap(), 2);
        assert_eq!(s.eval::<i64>("return GetNumQuestRewards()").unwrap(), 1);
        assert_eq!(s.eval::<i64>("return GetRewardMoney()").unwrap(), 1234);

        // Five values, `isUsable` 1 or nil, the type matched in any case.
        assert_eq!(s.arity(r#"GetQuestItemInfo("reward", 1)"#).unwrap(), 5);
        let (name, texture, count, quality, usable) = s
            .eval::<(String, String, i64, i64, i64)>(r#"return GetQuestItemInfo("REWARD", 1)"#)
            .unwrap();
        assert_eq!(name, "Brdle Leather Boots");
        assert_eq!(texture, "Interface\\Icons\\INV_Boots_01");
        assert_eq!((count, quality, usable), (1, 2, 1));

        // Choice row 2 is in flight: an empty name, the rest present.
        assert!(s
            .eval::<bool>(
                "local n, t, c = GetQuestItemInfo(\"choice\", 2)\n\
                 return n == '' and t == nil and c == 1"
            )
            .unwrap());
        // Inside 1..7 with no item: the zeroed row, display 0's question mark.
        assert_eq!(
            s.eval::<(String, String, i64, i64, i64)>(r#"return GetQuestItemInfo("reward", 7)"#)
                .unwrap(),
            (
                String::new(),
                "Interface\\Icons\\INV_Misc_QuestionMark".to_string(),
                0,
                0,
                1
            )
        );
        // Outside 1..7, an unknown type or a missing argument raises.
        for call in [
            r#"GetQuestItemInfo("reward", 8)"#,
            r#"GetQuestItemInfo("reward", 0)"#,
            r#"GetQuestItemInfo("bogus", 1)"#,
            r#"GetQuestItemInfo("reward")"#,
            "GetQuestItemInfo()",
        ] {
            let e = s.run(call).expect_err(call).to_string();
            assert!(
                e.contains(r#"Invalid quest item in GetQuestItemInfo("type", index)"#),
                "{call}: {e}"
            );
        }

        // The link is nil for the in-flight row, out of range, and for an unknown type.
        assert_eq!(
            s.eval::<String>("return GetQuestItemLink(\"reward\", 1)")
                .unwrap(),
            "|cff1eff00|Hitem:7278:0:0:0|h[Brdle Leather Boots]|h|r"
        );
        assert!(s
            .eval::<bool>("return GetQuestItemLink(\"choice\", 2) == nil")
            .unwrap());
        assert!(s
            .eval::<bool>("return GetQuestItemLink(\"reward\", 9) == nil")
            .unwrap());
        assert!(s
            .eval::<bool>("return GetQuestItemLink(\"bogus\", 1) == nil")
            .unwrap());
    }

    #[test]
    fn greeting_panel_lists_and_selects() {
        let mut s = UiScript::new().unwrap();
        let row = |quest_id, title: &str, level, active| GossipQuestRow {
            quest_id,
            title: title.into(),
            level,
            active,
            ..Default::default()
        };
        // Wire order interleaves the lists; each keeps its own 1-based order.
        s.set_quest(Some(QuestState {
            panel: QuestPanel::Greeting,
            greeting: "What can I do for you?".into(),
            greeting_quests: vec![
                GossipQuestRow {
                    daily: true,
                    ..row(61, "A Threat Within", 7, false)
                },
                row(60, "Report to Goldshire", 3, true),
                GossipQuestRow {
                    repeatable: true,
                    ..row(62, "Kobold Camp Cleanup", 9, false)
                },
                row(63, "Bounty on Murlocs", 9, true),
            ],
            ..Default::default()
        }));
        s.model_mut().player_req.level = 10;
        assert_eq!(
            s.eval::<String>("return GetGreetingText()").unwrap(),
            "What can I do for you?"
        );
        assert_eq!(s.eval::<i64>("return GetNumActiveQuests()").unwrap(), 2);
        assert_eq!(s.eval::<i64>("return GetNumAvailableQuests()").unwrap(), 2);
        assert_eq!(
            s.eval::<String>("return GetAvailableTitle(2)").unwrap(),
            "Kobold Camp Cleanup"
        );
        assert!(s
            .eval::<bool>("return GetAvailableTitle(3) == nil and GetAvailableTitle(0) == nil")
            .unwrap());
        // Level 3 at 10 is grey (3 + 5 < 10); level 9 is not.
        assert_eq!(s.eval::<i64>("return GetActiveLevel(1)").unwrap(), 3);
        assert_eq!(s.eval::<i64>("return GetAvailableLevel(9)").unwrap(), 0);
        assert!(s
            .eval::<bool>(
                "return IsActiveQuestTrivial(1) == 1 and IsActiveQuestTrivial(2) == nil \
                 and IsAvailableQuestTrivial(1) == nil"
            )
            .unwrap());
        assert_eq!(s.arity("GetAvailableQuestInfo(1)").unwrap(), 3);
        assert!(s
            .eval::<bool>(
                "local t1, d1, r1 = GetAvailableQuestInfo(1)\n\
                 local t2, d2, r2 = GetAvailableQuestInfo(2)\n\
                 return t1 == nil and d1 == 1 and r1 == nil and d2 == nil and r2 == 1"
            )
            .unwrap());
        assert!(s
            .run("GetActiveTitle('x')")
            .unwrap_err()
            .to_string()
            .contains("Usage: GetActiveTitle(index)"));

        // `isComplete` reads the log: quest 60's row is complete, and quest 63's holds no
        // objective and has not failed (`0x58bcb0`).
        s.set_quest_log(crate::script::QuestLogState {
            entries: vec![
                crate::script::QuestLogEntryView {
                    quest_id: 60,
                    complete: 1,
                    objectives: vec![Default::default()],
                    ..Default::default()
                },
                crate::script::QuestLogEntryView {
                    quest_id: 63,
                    no_objectives: true,
                    ..Default::default()
                },
            ],
            num_quests: 2,
            ..Default::default()
        });
        assert_eq!(
            s.eval::<(String, bool)>("return GetActiveTitle(1)")
                .unwrap(),
            ("Report to Goldshire".to_string(), true)
        );
        assert_eq!(
            s.eval::<(String, bool)>("return GetActiveTitle(2)")
                .unwrap(),
            ("Bounty on Murlocs".to_string(), true)
        );
        s.set_quest_log(crate::script::QuestLogState {
            entries: vec![crate::script::QuestLogEntryView {
                quest_id: 63,
                complete: -1,
                ..Default::default()
            }],
            num_quests: 1,
            ..Default::default()
        });
        assert!(
            !s.eval::<(String, bool)>("return GetActiveTitle(2)")
                .unwrap()
                .1,
            "a failed quest is not complete"
        );
        assert!(
            !s.eval::<(String, bool)>("return GetActiveTitle(1)")
                .unwrap()
                .1,
            "a quest off the log is not complete"
        );

        s.run("SelectActiveQuest(1)").unwrap();
        s.run("SelectAvailableQuest(2)").unwrap();
        assert_eq!(
            s.take_quest_selects(),
            vec![
                QuestSelect {
                    active: true,
                    index: 1
                },
                QuestSelect {
                    active: false,
                    index: 2
                },
            ]
        );
        assert!(s.take_quest_selects().is_empty(), "drained");
    }

    #[test]
    fn progress_panel_completability() {
        let mut s = UiScript::new().unwrap();
        s.set_quest(Some(QuestState {
            panel: QuestPanel::Progress,
            body: "Do you have the tusks?".into(),
            required: vec![QuestItemView {
                item_id: 0,
                name: Some("Chipped Boar Tusk".into()),
                texture: None,
                count: 8,
                quality: 0,
                usable: true,
                ..Default::default()
            }],
            required_money: 500,
            completable: true,
            ..Default::default()
        }));
        assert_eq!(
            s.eval::<String>("return GetProgressText()").unwrap(),
            "Do you have the tusks?"
        );
        assert_eq!(s.eval::<i64>("return GetNumQuestItems()").unwrap(), 1);
        assert_eq!(s.eval::<i64>("return GetQuestMoneyToGet()").unwrap(), 500);
        assert!(s.eval::<bool>("return IsQuestCompletable()").unwrap());
    }

    /// `0x58cff1`-`0x58cff9`: with choices on the reward panel, a choice out of range, or none,
    /// raises; a panel with no choices takes any.
    #[test]
    fn get_quest_reward_refuses_a_choice_out_of_range() {
        let mut s = UiScript::new().unwrap();
        s.set_quest(Some(QuestState {
            panel: QuestPanel::Reward,
            choices: vec![QuestItemView::default(), QuestItemView::default()],
            ..QuestState::default()
        }));
        for bad in ["GetQuestReward()", "GetQuestReward(0)", "GetQuestReward(3)"] {
            let err = s.run(bad).unwrap_err().to_string();
            assert!(
                err.contains("Invalid reward choice in GetQuestReward([choice])"),
                "{bad}: {err}"
            );
        }
        s.run("GetQuestReward(2)").unwrap();
        assert_eq!(s.take_quest_actions(), vec![QuestAction::Reward(1)]);
        s.set_quest(Some(QuestState {
            panel: QuestPanel::Reward,
            ..QuestState::default()
        }));
        s.run("GetQuestReward()").unwrap();
        assert_eq!(s.take_quest_actions(), vec![QuestAction::Reward(0)]);
    }

    #[test]
    fn button_intents_queue() {
        let mut s = UiScript::new().unwrap();
        s.set_quest(Some(detail()));
        s.run("AcceptQuest()").unwrap();
        assert_eq!(s.take_quest_actions(), vec![QuestAction::Accept]);

        s.run("CompleteQuest()").unwrap(); // progress -> reward
        s.run("GetQuestReward(2)").unwrap(); // row 2 (1-based) → wire choice 1 (0-based)
        s.run("GetQuestReward()").unwrap(); // no-choice quest -> 0
        s.run("DeclineQuest()").unwrap();
        assert_eq!(
            s.take_quest_actions(),
            vec![
                QuestAction::Continue,
                QuestAction::Reward(1),
                QuestAction::Reward(0),
                QuestAction::Decline,
            ]
        );
        assert!(s.take_quest_actions().is_empty(), "drained");
    }

    #[test]
    fn clearing_the_quest_empties_it() {
        let mut s = UiScript::new().unwrap();
        s.set_quest(Some(detail()));
        s.set_quest(None);
        assert_eq!(s.eval::<String>("return GetTitleText()").unwrap(), "");
        assert_eq!(s.eval::<i64>("return GetNumQuestRewards()").unwrap(), 0);
        assert!(s.eval::<bool>("return GetRewardSpell() == nil").unwrap());
    }

    #[test]
    fn decline_and_close_are_two_intents() {
        let mut s = UiScript::new().unwrap();
        s.set_quest(Some(detail()));
        s.run("DeclineQuest()").unwrap();
        s.run("CloseQuest()").unwrap();
        assert_eq!(
            s.take_quest_actions(),
            vec![QuestAction::Decline, QuestAction::Close]
        );
    }

    #[test]
    fn confirm_accept_quest_counts_and_drains() {
        let mut s = UiScript::new().unwrap();
        assert_eq!(s.take_quest_confirms(), 0);
        s.run("ConfirmAcceptQuest() ConfirmAcceptQuest()").unwrap();
        assert_eq!(s.take_quest_confirms(), 2);
        assert_eq!(s.take_quest_confirms(), 0, "drained");
    }
}
