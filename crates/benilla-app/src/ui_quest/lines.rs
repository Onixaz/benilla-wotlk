//! The quest chat lines the reference raises from its packet and field handlers: the accept line
//! from the quest-slot watcher, and the turn-in's completion, experience, money and item lines
//! (12340's `0x6d0e10`). A line whose quest or item record is not cached waits for it, as the
//! reference's cache callbacks print on the record's arrival.

use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use benilla_ui::script::UiScript;

use crate::items::Items;
use crate::net::{NetCommands, NetHandlerApp};
use crate::ui_action::{show_messages, ui_error_text, FillArg, MessageSink, Shown, UiError};
use crate::ui_quest_log::QuestLog;

/// One quest chat line, raised in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QuestLine {
    /// `ERR_QUEST_ACCEPTED_S` (`0x89`) with the quest's title.
    Accepted(u32),
    /// `ERR_QUEST_COMPLETE_S` with the title, for every turn-in; the next quest in the chain only
    /// decides whether the window stays (`0x6d10ca`).
    Completed(u32),
    /// `ERR_QUEST_REWARD_EXP_I` (`0x95`).
    Experience(u32),
    /// `ERR_QUEST_REWARD_MONEY_S` (`0x97`) with the coin string.
    Money(u32),
    /// `ERR_QUEST_REWARD_ITEM_S` with the item's link.
    Item(u32),
    /// The quest template's fixed reward items, one [`Self::Item`] each once the template is here
    /// (`0x6d1018`).
    RewardItems(u32),
}

/// The lines raised and not yet shown.
#[derive(Resource, Default)]
pub(crate) struct QuestLines(Vec<QuestLine>);

impl QuestLines {
    pub(crate) fn push(&mut self, line: QuestLine) {
        self.0.push(line);
    }

    #[cfg(test)]
    pub(crate) fn queued(&self) -> &[QuestLine] {
        &self.0
    }
}

/// What a line resolves to this frame.
enum Resolved {
    Show(UiError),
    /// Its record is still in flight.
    Wait,
    /// The server does not know the quest or item, or the line has nothing to say.
    Drop,
}

pub(super) fn register(app: &mut App) {
    app.init_resource::<QuestLines>()
        .net_handler(SessionEventKind::Disconnected, on_session_end)
        .add_systems(
            Update,
            feed_quest_lines
                .in_set(crate::ui_unit::UnitFeed)
                .after(crate::ui_quest_log::feed_quest_log),
        );
}

/// The waiting lines die with the socket, as the record caches do.
fn on_session_end(In(_): In<SessionEvent>, mut lines: ResMut<QuestLines>) {
    lines.0.clear();
}

/// Show every line whose record is here, in the order raised; the rest wait.
fn feed_quest_lines(
    script: Option<NonSendMut<UiScript>>,
    mut lines: ResMut<QuestLines>,
    quest_log: Res<QuestLog>,
    items: Res<Items>,
    commands: Res<NetCommands>,
    mut sink: MessageSink,
) {
    let Some(mut script) = script else {
        return;
    };
    if lines.0.is_empty() {
        return;
    }
    let get = |key: &str| benilla_ui::strings::global(script.lua(), key);
    // A template's reward items expand in place once its record is here.
    let mut expanded = Vec::with_capacity(lines.0.len());
    for line in lines.0.drain(..) {
        match line {
            QuestLine::RewardItems(id) => match quest(id, &quest_log, &commands) {
                Ok(t) => expanded.extend(
                    t.rewards
                        .iter()
                        .filter(|&&(item, _)| item != 0)
                        .map(|&(item, _)| QuestLine::Item(item)),
                ),
                Err(Resolved::Wait) => expanded.push(line),
                Err(_) => {}
            },
            other => expanded.push(other),
        }
    }
    lines.0 = expanded;
    let mut shown = Vec::new();
    let mut accepted = Vec::new();
    lines.0.retain(
        |&line| match resolve(line, &quest_log, &items, &commands, &get) {
            Resolved::Show(msg) => {
                if let Some(text) = ui_error_text(&msg, &get) {
                    shown.push(Shown::keyed(msg.key, text));
                }
                if let QuestLine::Accepted(id) = line {
                    accepted.push(id);
                }
                false
            }
            Resolved::Wait => true,
            Resolved::Drop => false,
        },
    );
    show_messages(&mut script, &mut sink, "ui_quest", shown);
    // `QUEST_ACCEPTED` with the quest's log row follows the accept line, both raised once the
    // template is cached (`QuestAcceptCallback` `0x6df2b0`, `OnUpdateQuest` `0x6df370`).
    for id in accepted {
        let index = script.quest_log_index(id) as i64;
        script.fire_event(
            "QUEST_ACCEPTED",
            vec![benilla_ui::script::ScriptValue::Int(index)],
        );
    }
}

fn resolve(
    line: QuestLine,
    quest_log: &QuestLog,
    items: &Items,
    commands: &NetCommands,
    get: &dyn Fn(&str) -> Option<String>,
) -> Resolved {
    match line {
        QuestLine::Accepted(id) => match quest(id, quest_log, commands) {
            Ok(t) => Resolved::Show(UiError::s("ERR_QUEST_ACCEPTED_S", t.title.clone())),
            Err(r) => r,
        },
        QuestLine::Completed(id) => match quest(id, quest_log, commands) {
            Ok(t) => Resolved::Show(UiError::s("ERR_QUEST_COMPLETE_S", t.title.clone())),
            Err(r) => r,
        },
        QuestLine::Experience(xp) => Resolved::Show(UiError::args(
            "ERR_QUEST_REWARD_EXP_I",
            vec![FillArg::D(i64::from(xp))],
        )),
        QuestLine::Money(copper) => Resolved::Show(UiError::s(
            "ERR_QUEST_REWARD_MONEY_S",
            coin_text(copper, &|k| get(k).unwrap_or_default()),
        )),
        // Expanded before resolving; one still here waits on its template.
        QuestLine::RewardItems(_) => Resolved::Wait,
        QuestLine::Item(entry) => match items.template(entry, 0, commands) {
            Some(t) => Resolved::Show(UiError::s(
                "ERR_QUEST_REWARD_ITEM_S",
                crate::ui_items::item_link(entry, &t.name, t.quality),
            )),
            None if items.template_answered_unknown(entry) => Resolved::Drop,
            None => Resolved::Wait,
        },
    }
}

/// The quest's record, or what its line does without one.
fn quest<'a>(
    id: u32,
    quest_log: &'a QuestLog,
    commands: &NetCommands,
) -> Result<&'a benilla_protocol::messages::QuestTemplate, Resolved> {
    match quest_log.template(id, commands) {
        Some(t) => Ok(t),
        None if quest_log.template_answered_unknown(id) => Err(Resolved::Drop),
        None => Err(Resolved::Wait),
    }
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

    /// The feed alone, over a VM holding the stock enUS templates.
    fn app() -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.insert_resource(NetCommands(tx))
            .init_resource::<QuestLines>()
            .init_resource::<QuestLog>()
            .init_resource::<Items>()
            .init_resource::<crate::ui_chat::ChatLog>()
            .init_resource::<crate::sound::MessageSounds>()
            .add_systems(Update, feed_quest_lines);
        let script = UiScript::new().unwrap();
        script
            .run(
                r#"
                ERR_QUEST_ACCEPTED_S = "Quest accepted: %s"
                ERR_QUEST_COMPLETE_S = "%s completed."
                ERR_QUEST_REWARD_EXP_I = "Experience gained: %d."
                ERR_QUEST_REWARD_MONEY_S = "Received %s."
                ERR_QUEST_REWARD_ITEM_S = "Received item: %s."
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

    #[test]
    fn each_line_shows_as_its_record_lands_and_the_rest_keep_their_order() {
        let (mut app, rx) = app();
        {
            let world = app.world_mut();
            world
                .resource_mut::<QuestLog>()
                .insert_template(template(783, "A Threat Within", 0));
            world
                .resource_mut::<Items>()
                .insert_template(2_580, Some(crate::items::test_template("Linen Cloth")));
            let mut lines = world.resource_mut::<QuestLines>();
            for line in [
                QuestLine::Accepted(783),
                QuestLine::Completed(7),
                QuestLine::Experience(450),
                QuestLine::Money(1_025),
                QuestLine::Item(2_580),
                QuestLine::Item(6_529),
            ] {
                lines.push(line);
            }
        }
        app.update();
        assert_eq!(
            chat(&app),
            [
                "Quest accepted: A Threat Within",
                "Experience gained: 450.",
                "Received 10 Silver, 25 Copper.",
                "Received item: |cffffffff|Hitem:2580:0:0:0|h[Linen Cloth]|h|r.",
            ]
        );
        assert_eq!(
            app.world().resource::<QuestLines>().queued(),
            [QuestLine::Completed(7), QuestLine::Item(6_529)],
            "the uncached quest and item wait"
        );
        let sent: Vec<_> = rx.try_iter().collect();
        assert!(
            sent.iter()
                .any(|c| matches!(c, ClientCommand::QuestQuery { quest: 7 })),
            "the waiting quest's record is asked: {sent:?}"
        );
        assert!(
            app.world()
                .resource::<crate::sound::MessageSounds>()
                .queued()
                .iter()
                .any(|r| r.key == "ERR_QUEST_ACCEPTED_S"),
            "the accept line's row carries its sound"
        );

        {
            let world = app.world_mut();
            world
                .resource_mut::<QuestLog>()
                .insert_template(template(7, "Kobold Camp Cleanup", 0));
            world
                .resource_mut::<Items>()
                .insert_template(6_529, Some(crate::items::test_template("Shiny Bauble")));
        }
        app.update();
        assert_eq!(
            chat(&app)[4..],
            [
                "Kobold Camp Cleanup completed.",
                "Received item: |cffffffff|Hitem:6529:0:0:0|h[Shiny Bauble]|h|r.",
            ]
        );
        assert!(app.world().resource::<QuestLines>().queued().is_empty());
    }

    /// 12340 names every turn-in, a chained quest too (`0x6d0e10`); an unknown item says nothing.
    #[test]
    fn a_chained_quest_still_completes_aloud_and_an_unknown_item_shows_nothing() {
        let (mut app, _rx) = app();
        {
            let world = app.world_mut();
            world
                .resource_mut::<QuestLog>()
                .insert_template(template(783, "A Threat Within", 7));
            world.resource_mut::<Items>().insert_template(9_999, None);
            let mut lines = world.resource_mut::<QuestLines>();
            lines.push(QuestLine::Completed(783));
            lines.push(QuestLine::Item(9_999));
        }
        app.update();
        assert_eq!(chat(&app), ["A Threat Within completed."]);
        assert!(app.world().resource::<QuestLines>().queued().is_empty());
    }

    /// The fixed rewards print from the template once it lands, one item line each (`0x6d1018`).
    #[test]
    fn a_templates_reward_items_expand_when_the_template_lands() {
        let (mut app, _rx) = app();
        {
            let world = app.world_mut();
            world
                .resource_mut::<Items>()
                .insert_template(6_529, Some(crate::items::test_template("Shiny Bauble")));
            world
                .resource_mut::<QuestLines>()
                .push(QuestLine::RewardItems(7));
        }
        app.update();
        assert!(chat(&app).is_empty());
        assert_eq!(
            app.world().resource::<QuestLines>().queued(),
            [QuestLine::RewardItems(7)],
            "the template is still in flight"
        );
        {
            let mut t = template(7, "Kobold Camp Cleanup", 0);
            t.rewards[1] = (6_529, 1);
            app.world_mut()
                .resource_mut::<QuestLog>()
                .insert_template(t);
        }
        app.update();
        assert_eq!(
            chat(&app),
            ["Received item: |cffffffff|Hitem:6529:0:0:0|h[Shiny Bauble]|h|r."]
        );
        assert!(app.world().resource::<QuestLines>().queued().is_empty());
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
