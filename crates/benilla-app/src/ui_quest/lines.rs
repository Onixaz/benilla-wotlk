//! The quest chat lines the reference raises from its packet and field handlers: the accept line
//! from the quest-slot watch ([`accept_line`]), and the turn-in's lines ([`turn_in`], `0x6d0e10`), run by the complete handler with its template cached or by the template's answer.

use benilla_protocol::messages::QuestComplete;
use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use benilla_ui::script::{ScriptValue, UiScript};

use super::QuestGiver;
use crate::items::Items;
use crate::net::{NetCommands, NetHandlerApp};
use crate::ui_action::{
    show_in_turn, show_messages, ui_error_text, FillArg, MessageSink, Shown, UiError,
};
use crate::ui_quest_log::QuestLog;
use crate::ui_script::live::UiEvents;

/// The turn-ins waiting on their quest's template (`0x6d1192`); a reward item line waits on [`crate::items::RecordWaits`].
#[derive(Resource, Default)]
pub(crate) struct TurnIns {
    quests: Vec<QuestComplete>,
}

impl TurnIns {
    /// A turn-in whose template the complete handler asked for.
    pub(crate) fn wait_for_template(&mut self, c: QuestComplete) {
        self.quests.push(c);
    }

    #[cfg(test)]
    pub(crate) fn waiting(&self) -> usize {
        self.quests.len()
    }
}

pub(super) fn register(app: &mut App) {
    app.init_resource::<TurnIns>()
        .net_handler(SessionEventKind::Disconnected, on_session_end);
}

/// The waiting turn-ins and lines die with the socket, as the record caches do.
fn on_session_end(In(_): In<SessionEvent>, mut turn_ins: ResMut<TurnIns>) {
    *turn_ins = TurnIns::default();
}

/// A reward line's callback on its item's answer (`0x6cd8c0`); an unknown item prints nothing (`0x6cd8c6`).
pub(crate) fn reward_landed(
    ui: &mut UiEvents,
    entry: u32,
    info: Option<&benilla_protocol::ItemInfo>,
    count: u32,
) {
    if let Some(info) = info {
        let link = crate::ui_items::item_link(entry, &info.name, info.quality);
        show_in_turn(ui, "ui_quest", reward_item_line(&link, count));
    }
}

/// A reward item's line (`0x6cd90d`): `ERR_QUEST_REWARD_ITEM_MULT_IS` past one, else `ERR_QUEST_REWARD_ITEM_S`.
fn reward_item_line(link: &str, count: u32) -> UiError {
    if count > 1 {
        UiError::args(
            "ERR_QUEST_REWARD_ITEM_MULT_IS",
            vec![FillArg::D(i64::from(count)), FillArg::S(link.to_string())],
        )
    } else {
        UiError::s("ERR_QUEST_REWARD_ITEM_S", link)
    }
}

/// The template answer runs every turn-in that waited on it (`0x6d1175`).
pub(crate) fn quest_landed(world: &mut World, quest_id: u32) {
    let Some(mut turn_ins) = world.get_resource_mut::<TurnIns>() else {
        return;
    };
    let mut ready = Vec::new();
    turn_ins.quests.retain(|c| {
        if c.quest_id == quest_id {
            ready.push(*c);
        }
        c.quest_id != quest_id
    });
    for c in ready {
        turn_in(world, c);
    }
}

/// The turn-in (`0x6d0e10`): its lines, the chosen reward's latch read and zeroed (`0x6d1061`, `0x6d10bc`), then the window closes unless the quest chains on (`0x6d10ca`, `0x6d10d4`).
pub(crate) fn turn_in(world: &mut World, c: QuestComplete) {
    let chosen = world
        .get_non_send_resource_mut::<QuestGiver>()
        .map_or(0, |mut g| std::mem::take(&mut g.chosen_reward));
    match world.run_system_cached_with(turn_in_lines, (c, chosen)) {
        Ok(Some(chained)) => super::giver_finished(world, chained, c.quest_id, false),
        Ok(None) => {}
        Err(e) => warn!("ui_quest: the turn-in did not run: {e}"),
    }
}

/// Show one line through the VM's own strings.
fn say(script: &mut UiScript, sink: &mut MessageSink, msg: UiError) {
    let get = |key: &str| benilla_ui::strings::global(script.lua(), key);
    let shown = ui_error_text(&msg, &get).map(|text| Shown::keyed(msg.key, text));
    show_messages(script, sink, "ui_quest", shown);
}

/// The turn-in's lines in 12340's order (`0x6d0e66`-`0x6d10b4`); whether the quest chains on, `None` with no template.
fn turn_in_lines(
    In((c, chosen)): In<(QuestComplete, u32)>,
    (script, mut live): (Option<NonSendMut<UiScript>>, crate::ui_script::LendLive),
    (quest_log, items, commands): (Res<QuestLog>, Res<Items>, Res<NetCommands>),
    mut sink: MessageSink,
    mut center: Option<MessageWriter<crate::ui_unit::CombatTextEvent>>,
) -> Option<bool> {
    let t = quest_log.template(c.quest_id, &commands)?;
    let chained = t.next_quest_in_chain != 0;
    let Some(mut script) = script else {
        return Some(chained);
    };
    say(
        &mut script,
        &mut sink,
        UiError::s("ERR_QUEST_COMPLETE_S", t.title.clone()),
    );
    script.queue_sound_kit("igQuestListComplete");
    if c.xp != 0 {
        say(
            &mut script,
            &mut sink,
            UiError::args("ERR_QUEST_REWARD_EXP_I", vec![FillArg::D(i64::from(c.xp))]),
        );
    }
    // Signed tests from here on (`0x6d0ea3`, `0x6d0ed5`, `0x6d0f60`, `0x6d0fb1`).
    if c.money as i32 > 0 {
        let coins = {
            let get = |k: &str| benilla_ui::strings::global(script.lua(), k).unwrap_or_default();
            coin_text(c.money, &get)
        };
        say(
            &mut script,
            &mut sink,
            UiError::s("ERR_QUEST_REWARD_MONEY_S", coins),
        );
    }
    let mut text = |message_type: &'static str, n: u32| {
        if let Some(center) = center.as_mut() {
            center.write(crate::ui_unit::CombatTextEvent {
                message_type,
                args: vec![ScriptValue::Int(i64::from(n))],
            });
        }
    };
    // The honor, a tenth of the wire's: `COMBATLOG_HONORAWARD` and `HONOR_GAINED` (`0x6d0edb`-`0x6d0f55`).
    if c.honor as i32 > 0 {
        let honor = c.honor / 10;
        sink.chat.push_pvp_credit(honor as i32, 0, 0);
        text("HONOR_GAINED", honor);
    }
    // `LEVEL_UP_CHAR_POINTS` as a `CHAT_MSG_MONEY` line (`0x6d0f5d`-`0x6d0fa6`).
    if c.talents as i32 > 0 {
        let msg = UiError::args(
            "LEVEL_UP_CHAR_POINTS",
            vec![FillArg::D(i64::from(c.talents))],
        );
        let get = |key: &str| benilla_ui::strings::global(script.lua(), key);
        if let Some(line) = ui_error_text(&msg, &get) {
            sink.chat.push_event(crate::ui_chat::ChatEvent::text_only(
                crate::ui_chat::ChatEventKind::Money,
                line,
            ));
        }
    }
    // Deviation: the arena award's chat line goes out as type `0x32` (`0x6d0ff4`), which has no `ChatTypeInfo` row in the stock UI, so only its text is raised (`0x6d1005`).
    if c.arena_points as i32 > 0 {
        text("ARENA_POINTS_GAINED", c.arena_points);
    }
    // The template's fixed rewards, then the chosen one with no count (`0x6d1018`-`0x6d10b4`).
    let rewards: Vec<(u32, u32)> = t
        .rewards
        .iter()
        .filter(|&&(item, _)| item as i32 > 0)
        .copied()
        .chain((chosen != 0).then_some((chosen, 0)))
        .collect();
    for (item, count) in rewards {
        match items.template(item, 0, &commands) {
            Some(it) => {
                let link = crate::ui_items::item_link(item, &it.name, it.quality);
                say(&mut script, &mut sink, reward_item_line(&link, count));
            }
            None if items.template_answered_unknown(item) => {}
            None => live.wait_record(item, crate::items::RecordWait::QuestReward(count)),
        }
    }
    Some(chained)
}

/// The accept's line and tutorials, its template cached (`0x6df440`-`0x6df481`, the answer's `0x6df2f9`-`0x6df343`).
#[allow(clippy::too_many_arguments)] // one Bevy system's full input set
pub(crate) fn accept_line(
    In(quest_id): In<u32>,
    (script, _live): (Option<NonSendMut<UiScript>>, crate::ui_script::LendLive),
    quest_log: Res<QuestLog>,
    commands: Res<NetCommands>,
    mut sink: MessageSink,
    tutorial_bank: Option<Res<crate::tutorial::Tutorials>>,
    mut tutorials: Option<MessageWriter<crate::tutorial::TutorialEvent>>,
    // The accept callback's process-wide count (`0x6df46d`).
    mut accepts: Local<u32>,
) {
    let Some(mut script) = script else {
        return;
    };
    let Some(t) = quest_log.template(quest_id, &commands) else {
        return;
    };
    let get = |key: &str| benilla_ui::strings::global(script.lua(), key);
    let msg = UiError::s("ERR_QUEST_ACCEPTED_S", t.title.clone());
    let shown: Vec<Shown> = ui_error_text(&msg, &get)
        .map(|text| Shown::keyed(msg.key, text))
        .into_iter()
        .collect();
    show_messages(&mut script, &mut sink, "ui_quest", shown);
    let flagged = tutorial_bank
        .as_deref()
        .is_some_and(|t| t.is_flagged(crate::tutorial::id::QUEST_LOG));
    for tutorial in accept_tutorials(t.quest_type, flagged, &mut accepts) {
        if let Some(t) = tutorials.as_mut() {
            t.write(crate::tutorial::TutorialEvent::trigger(tutorial));
        }
    }
}

/// The accept callback's tutorials (`0x6df448`-`0x6df481`): a quest of type 1 raises Group Quest
/// then Groups, and from the fourth accept on the Quest Log one; the count stops once the Quest Log
/// tutorial is acknowledged.
fn accept_tutorials(quest_type: u32, quest_log_flagged: bool, accepts: &mut u32) -> Vec<u32> {
    use crate::tutorial::id;
    let mut out = Vec::new();
    if quest_type == 1 {
        out.extend([id::GROUP_QUEST, id::GROUPS]);
    }
    if !quest_log_flagged {
        *accepts = accepts.saturating_add(1);
        if *accepts >= 4 {
            out.push(id::QUEST_LOG);
        }
    }
    out
}

/// The turn-in's coin string, 12340's `0x7e7d80`: each paid denomination through its
/// `GOLD_AMOUNT`, `SILVER_AMOUNT` or `COPPER_AMOUNT` format (`0xaf4914`), gold first, joined by
/// `", "` (`0x9fc494`).
pub(crate) fn coin_text(copper: u32, get: &dyn Fn(&str) -> String) -> String {
    let (g, s, c) = (copper / 10000, copper / 100 % 100, copper % 100);
    [
        (g, "GOLD_AMOUNT"),
        (s, "SILVER_AMOUNT"),
        (c, "COPPER_AMOUNT"),
    ]
    .into_iter()
    .filter(|&(n, _)| n != 0)
    .map(|(n, key)| get(key).replacen("%d", &n.to_string(), 1))
    .collect::<Vec<_>>()
    .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::messages::QuestTemplate;

    use crate::net::ClientCommand;

    fn template(quest_id: u32, title: &str, next_quest_in_chain: u32) -> QuestTemplate {
        QuestTemplate {
            quest_id,
            method: 2,
            level: 1,
            zone_or_sort: 12,
            next_quest_in_chain,
            title: title.into(),
            ..Default::default()
        }
    }

    /// The turn-in's world, over a VM holding the stock enUS templates.
    fn app() -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.insert_resource(NetCommands(tx))
            .init_resource::<TurnIns>()
            .init_non_send_resource::<QuestGiver>()
            .init_resource::<QuestLog>()
            .init_resource::<crate::ui_quest_log::QuestLogMarks>()
            .init_non_send_resource::<crate::ui_quest_log::QuestExpiries>()
            .init_resource::<Items>()
            .init_non_send_resource::<crate::items::RecordWaits>()
            .init_resource::<UiEvents>()
            .init_resource::<crate::ui_chat::ChatLog>()
            .init_resource::<crate::sound::MessageSounds>()
            .add_message::<crate::ui_unit::CombatTextEvent>();
        let script = UiScript::new().unwrap();
        script
            .run(
                r#"
                ERR_QUEST_ACCEPTED_S = "Quest accepted: %s"
                ERR_QUEST_COMPLETE_S = "%s completed."
                ERR_QUEST_REWARD_EXP_I = "Experience gained: %d."
                ERR_QUEST_REWARD_MONEY_S = "Received %s."
                ERR_QUEST_REWARD_ITEM_S = "Received item: %s."
                ERR_QUEST_REWARD_ITEM_MULT_IS = "Received %d of item: %s."
                LEVEL_UP_CHAR_POINTS = "You have gained %d talent points."
                GOLD_AMOUNT = "%d Gold"; SILVER_AMOUNT = "%d Silver"; COPPER_AMOUNT = "%d Copper"
                "#,
            )
            .unwrap();
        app.insert_non_send_resource(script);
        (app, rx)
    }

    fn chat(app: &App) -> Vec<String> {
        app.world()
            .resource::<crate::ui_chat::ChatLog>()
            .pending_lines()
    }

    fn complete(quest_id: u32) -> QuestComplete {
        QuestComplete {
            quest_id,
            ..Default::default()
        }
    }

    /// An item answer through its handler, with the lines it shows in turn.
    /// The item answer's reward-line callbacks, as the object handler runs them.
    fn item_answer(app: &mut App, entry: u32, name: Option<&str>) {
        let info = name.map(crate::items::test_template);
        let world = app.world_mut();
        let waits = world
            .non_send_resource_mut::<crate::items::RecordWaits>()
            .take(entry);
        for wait in waits {
            if let crate::items::RecordWait::QuestReward(count) = wait {
                let mut ui = world.resource_mut::<UiEvents>();
                reward_landed(&mut ui, entry, info.as_ref(), count);
            }
        }
        crate::ui_script::live::fire_handler_events(world);
    }

    fn reward_waits(app: &App) -> Vec<(u32, crate::items::RecordWait)> {
        app.world()
            .non_send_resource::<crate::items::RecordWaits>()
            .waiting()
            .to_vec()
    }

    /// `0x6d0e10`'s order: the completion, the fanfare, the XP, the money, the talents, the template's rewards (a count past one says so, `0x6cd90d`), then the chosen reward, which the turn-in spends; an uncached item's line waits for its answer.
    #[test]
    fn a_turn_in_prints_in_order_and_an_uncached_item_waits_for_its_answer() {
        let (mut app, rx) = app();
        {
            let world = app.world_mut();
            let mut t = template(7, "Kobold Camp Cleanup", 0);
            t.rewards[0] = (2_580, 1);
            t.rewards[1] = (6_529, 3);
            world.resource_mut::<QuestLog>().insert_template(t);
            world
                .resource_mut::<Items>()
                .insert_template(2_580, Some(crate::items::test_template("Linen Cloth")));
            world.resource_mut::<Items>().insert_template(
                2_047,
                Some(crate::items::test_template("Anvilmar Hand Axe")),
            );
            world.non_send_resource_mut::<QuestGiver>().chosen_reward = 2_047;
            turn_in(
                world,
                QuestComplete {
                    quest_id: 7,
                    xp: 450,
                    money: 1_025,
                    talents: 1,
                    ..Default::default()
                },
            );
        }
        assert_eq!(
            chat(&app),
            [
                "Kobold Camp Cleanup completed.",
                "Experience gained: 450.",
                "Received 10 Silver, 25 Copper.",
                "You have gained 1 talent points.",
                "Received item: |cffffffff|Hitem:2580:0:0:0:0:0:0:0:0|h[Linen Cloth]|h|r.",
                "Received item: |cffffffff|Hitem:2047:0:0:0:0:0:0:0:0|h[Anvilmar Hand Axe]|h|r.",
            ]
        );
        assert_eq!(
            app.world_mut()
                .non_send_resource_mut::<UiScript>()
                .take_sounds(),
            [benilla_ui::script::SoundRequest::KitName(
                "igQuestListComplete".into()
            )]
        );
        assert_eq!(
            app.world().non_send_resource::<QuestGiver>().chosen_reward,
            0
        );
        assert_eq!(
            reward_waits(&app),
            [(6_529, crate::items::RecordWait::QuestReward(3))]
        );
        assert!(rx
            .try_iter()
            .any(|c| matches!(c, ClientCommand::ItemQuery { entry: 6_529, .. })));
        item_answer(&mut app, 6_529, Some("Shiny Bauble"));
        assert_eq!(
            chat(&app)[6..],
            ["Received 3 of item: |cffffffff|Hitem:6529:0:0:0:0:0:0:0:0|h[Shiny Bauble]|h|r."]
        );
        assert_eq!(app.world().resource::<TurnIns>().waiting(), 0);
        assert!(reward_waits(&app).is_empty());
    }

    /// An unknown item's answer prints nothing (`0x6cd8c6`).
    #[test]
    fn an_unknown_items_answer_drops_its_line() {
        let (mut app, _rx) = app();
        {
            let world = app.world_mut();
            let mut t = template(7, "Kobold Camp Cleanup", 0);
            t.rewards[0] = (9_999, 1);
            world.resource_mut::<QuestLog>().insert_template(t);
            turn_in(world, complete(7));
        }
        item_answer(&mut app, 9_999, None);
        assert_eq!(chat(&app), ["Kobold Camp Cleanup completed."]);
        assert_eq!(app.world().resource::<TurnIns>().waiting(), 0);
        assert!(reward_waits(&app).is_empty());
    }

    /// With the template uncached the whole turn-in waits for its answer, the window too (`0x6d1192`); the answer runs it (`0x6d1175`).
    #[test]
    fn a_turn_in_waits_for_its_template_and_the_answer_runs_it() {
        let (mut app, _rx) = app();
        app.world_mut().non_send_resource_mut::<QuestGiver>().open(
            0xF130_0000_0000_0007,
            crate::ui_quest::QuestView::Greeting(Default::default()),
        );
        app.world_mut()
            .resource_mut::<TurnIns>()
            .wait_for_template(complete(7));
        assert!(chat(&app).is_empty());
        assert!(app.world().non_send_resource::<QuestGiver>().is_open());
        let world = app.world_mut();
        world
            .resource_mut::<QuestLog>()
            .insert_template(template(7, "Kobold Camp Cleanup", 0));
        quest_landed(world, 7);
        assert_eq!(chat(&app), ["Kobold Camp Cleanup completed."]);
        assert!(!app.world().non_send_resource::<QuestGiver>().is_open());
        assert_eq!(app.world().resource::<TurnIns>().waiting(), 0);
        assert!(reward_waits(&app).is_empty());
    }

    /// `0x6d10ca`: a chained turn-in leaves the window to the next quest's panel and still prints.
    #[test]
    fn a_chained_turn_in_keeps_the_window_and_a_last_one_closes_it() {
        for (next, open) in [(8, true), (0, false)] {
            let (mut app, _rx) = app();
            let world = app.world_mut();
            world.non_send_resource_mut::<QuestGiver>().open(
                0xF130_0000_0000_0007,
                crate::ui_quest::QuestView::Greeting(Default::default()),
            );
            world
                .resource_mut::<QuestLog>()
                .insert_template(template(7, "A Threat Within", next));
            turn_in(world, complete(7));
            assert_eq!(chat(&app), ["A Threat Within completed."]);
            assert_eq!(
                app.world().non_send_resource::<QuestGiver>().is_open(),
                open
            );
        }
    }

    fn triggered(app: &mut App) -> Vec<u32> {
        app.world_mut()
            .resource_mut::<Messages<crate::tutorial::TutorialEvent>>()
            .drain()
            .filter_map(|e| match e {
                crate::tutorial::TutorialEvent::Trigger { id, .. } => Some(id),
                _ => None,
            })
            .collect()
    }

    /// 12340 raises the Quest Log tutorial from the accept callback on the fourth accept, never on
    /// the click (`0x6df341`), and a group quest's two with every accept (`0x6df307`).
    #[test]
    fn the_quest_log_tutorial_waits_for_the_fourth_accept() {
        use crate::tutorial::id;
        let (mut app, _rx) = app();
        app.add_message::<crate::tutorial::TutorialEvent>();
        let mut group = template(9, "Group Quest", 0);
        group.quest_type = 1;
        {
            let world = app.world_mut();
            let mut log = world.resource_mut::<QuestLog>();
            for q in 1..=3 {
                log.insert_template(template(q, "Solo Quest", 0));
            }
            log.insert_template(group);
        }
        let mut seen = Vec::new();
        for q in [1, 2, 3, 9] {
            app.world_mut()
                .run_system_cached_with(accept_line, q)
                .unwrap();
            app.update();
            seen.push(triggered(&mut app));
        }
        assert_eq!(
            seen,
            [
                vec![],
                vec![],
                vec![],
                vec![id::GROUP_QUEST, id::GROUPS, id::QUEST_LOG]
            ]
        );
    }

    #[test]
    fn an_acknowledged_quest_log_tutorial_stops_the_count() {
        use crate::tutorial::id;
        let mut n = 0;
        for _ in 0..3 {
            assert!(accept_tutorials(0, false, &mut n).is_empty());
        }
        assert!(
            accept_tutorials(0, true, &mut n).is_empty(),
            "flagged: no count"
        );
        assert_eq!(n, 3);
        assert_eq!(accept_tutorials(0, false, &mut n), [id::QUEST_LOG]);
        assert_eq!(accept_tutorials(0, false, &mut n), [id::QUEST_LOG]);
    }

    fn words(key: &str) -> String {
        match key {
            "GOLD_AMOUNT" => "%d Gold",
            "SILVER_AMOUNT" => "%d Silver",
            "COPPER_AMOUNT" => "%d Copper",
            _ => "",
        }
        .into()
    }

    #[test]
    fn the_coin_string_joins_only_the_paid_denominations() {
        assert_eq!(coin_text(5, &words), "5 Copper");
        assert_eq!(coin_text(250, &words), "2 Silver, 50 Copper");
        assert_eq!(coin_text(10_200, &words), "1 Gold, 2 Silver");
        assert_eq!(coin_text(12_345, &words), "1 Gold, 23 Silver, 45 Copper");
        assert_eq!(coin_text(30_000, &words), "3 Gold");
        // A zero middle coin leaves no empty place between its neighbours.
        assert_eq!(coin_text(10_005, &words), "1 Gold, 5 Copper");
    }
}
