//! The questgiver packet handlers: each fills the [`QuestGiver`] the feed reads, and each panel
//! packet replaces the open view.

use benilla_protocol::messages::{
    QuestComplete, QuestDetails, QuestGiverList, QuestOfferReward, QuestRequestItems, QuestShareMsg,
};
use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::{QuestGiver, QuestLine, QuestLines};
use crate::creature_anim::QueueNpcEmotes;
use crate::net::{ClientCommand, NetCommands, NetHandlerApp};
use crate::ui_action::UiError;
use crate::ui_quest_log::QuestLog;

/// One handler per kind, plus the session-end and world-enter listeners.
pub(super) fn register(app: &mut App) {
    use SessionEventKind as K;
    app.net_handler(K::QuestGiverStatus, on_giver_status)
        .net_handler(K::QuestGreeting, on_greeting)
        .net_handler(K::QuestDetail, on_detail)
        .net_handler(K::QuestProgress, on_progress)
        .net_handler(K::QuestOffer, on_offer)
        .net_handler(K::QuestComplete, on_complete)
        .net_handler(K::QuestObjectiveKill, on_objective_kill)
        .net_handler(K::QuestObjectiveItem, on_objective_item)
        .net_handler(K::QuestObjectivesComplete, on_objectives_complete)
        .net_handler(K::QuestFailed, on_failed)
        .net_handler(K::QuestLogFull, on_log_full)
        .net_handler(K::QuestGiverInvalid, on_giver_invalid)
        .net_handler(K::QuestGiverFailed, on_giver_failed)
        .net_handler(K::Disconnected, on_session_end)
        .net_handler(K::Connected, on_login)
        .net_handler(K::Worldport, on_worldport);
}

/// The reference's world-enter reset, run by the enter-world cascade on a login and on every
/// worldport, is the latch's only clear.
fn on_login(In(ev): In<SessionEvent>, mut quest: ResMut<QuestGiver>) {
    if let SessionEvent::Connected { .. } = ev {
        world_enter_reset(&mut quest);
    }
}

/// The same reset on a worldport.
fn on_worldport(In(ev): In<SessionEvent>, mut quest: ResMut<QuestGiver>) {
    if let SessionEvent::Worldport { .. } = ev {
        world_enter_reset(&mut quest);
    }
}

fn world_enter_reset(quest: &mut QuestGiver) {
    quest.close_on_cancel = 0;
    // The same reset zeroes the chosen reward.
    quest.chosen_reward = 0;
}

fn on_giver_status(In(ev): In<SessionEvent>, mut quest: ResMut<QuestGiver>) {
    if let SessionEvent::QuestGiverStatus { npc, status } = ev {
        quest_giver_status(npc, status, &mut quest);
    }
}

/// The panel packets' emote writer; absent without the animation plugin.
type EmoteOut<'w> = Option<MessageWriter<'w, QueueNpcEmotes>>;

/// The emote pairs a panel packet plays on its NPC: the greeting and progress readers queue their
/// one pair (`0x6d0447`, `0x6d0a95`), the details reader its run unless a player shares it
/// (`0x6d0740`), the reward reader its run (`0x6d0b8f`).
fn panel_emotes(ev: &SessionEvent) -> Option<QueueNpcEmotes> {
    let (guid, emotes) = match ev {
        SessionEvent::QuestGreeting(l) => (l.npc, vec![(l.emote_delay, l.emote)]),
        SessionEvent::QuestDetail(d) if !benilla_protocol::guid::is_player(d.npc) => {
            (d.npc, d.emotes.clone())
        }
        SessionEvent::QuestProgress(p) => (p.npc, vec![(p.emote_delay, p.emote)]),
        SessionEvent::QuestOffer(o) => (o.npc, o.emotes.clone()),
        _ => return None,
    };
    Some(QueueNpcEmotes { guid, emotes })
}

fn queue_panel_emotes(ev: &SessionEvent, out: &mut EmoteOut) {
    if let (Some(ask), Some(out)) = (panel_emotes(ev), out.as_mut()) {
        out.write(ask);
    }
}

fn on_greeting(In(ev): In<SessionEvent>, mut quest: ResMut<QuestGiver>, mut emotes: EmoteOut) {
    queue_panel_emotes(&ev, &mut emotes);
    if let SessionEvent::QuestGreeting(list) = ev {
        quest_greeting(list, &mut quest);
    }
}

fn on_detail(
    In(ev): In<SessionEvent>,
    mut quest: ResMut<QuestGiver>,
    commands: Res<NetCommands>,
    mut emotes: EmoteOut,
) {
    queue_panel_emotes(&ev, &mut emotes);
    if let SessionEvent::QuestDetail(d) = ev {
        quest_detail(d, &mut quest, &commands);
    }
}

fn on_progress(In(ev): In<SessionEvent>, mut quest: ResMut<QuestGiver>, mut emotes: EmoteOut) {
    queue_panel_emotes(&ev, &mut emotes);
    if let SessionEvent::QuestProgress(p) = ev {
        quest_progress(p, &mut quest);
    }
}

fn on_offer(In(ev): In<SessionEvent>, mut quest: ResMut<QuestGiver>, mut emotes: EmoteOut) {
    queue_panel_emotes(&ev, &mut emotes);
    if let SessionEvent::QuestOffer(o) = ev {
        quest_offer(o, &mut quest);
    }
}

fn on_complete(
    In(ev): In<SessionEvent>,
    mut quest: ResMut<QuestGiver>,
    mut lines: ResMut<QuestLines>,
    mut chat: ResMut<crate::ui_chat::ChatLog>,
    mut center: MessageWriter<crate::ui_unit::CombatTextEvent>,
    (quest_log, commands): (Res<QuestLog>, Res<NetCommands>),
) {
    if let SessionEvent::QuestComplete(c) = ev {
        turn_in_awards(&c, &mut chat, &mut center);
        // A quest with a next in its chain keeps the window up for that quest's panel.
        let chained = quest_log
            .template(c.quest_id, &commands)
            .is_some_and(|t| t.next_quest_in_chain != 0);
        quest_complete(c, chained, &mut quest, &mut lines);
    }
}

/// The turn-in's honor and arena awards (`0x6d0ed0`-`0x6d0fd0`): the honor, a tenth of the
/// wire's, prints `COMBATLOG_HONORAWARD` and sends `HONOR_GAINED`; the arena points send
/// `ARENA_POINTS_GAINED`.
fn turn_in_awards(
    c: &QuestComplete,
    chat: &mut crate::ui_chat::ChatLog,
    center: &mut MessageWriter<crate::ui_unit::CombatTextEvent>,
) {
    use benilla_ui::script::ScriptValue;
    let honor = c.honor / 10;
    if c.honor as i32 > 0 {
        chat.push_pvp_credit(honor as i32, 0, 0);
        center.write(crate::ui_unit::CombatTextEvent {
            message_type: "HONOR_GAINED",
            args: vec![ScriptValue::Int(i64::from(honor))],
        });
    }
    if c.arena_points as i32 > 0 {
        center.write(crate::ui_unit::CombatTextEvent {
            message_type: "ARENA_POINTS_GAINED",
            args: vec![ScriptValue::Int(i64::from(c.arena_points))],
        });
    }
}

fn on_objective_kill(In(ev): In<SessionEvent>, mut quest: ResMut<QuestGiver>) {
    if let SessionEvent::QuestObjectiveKill {
        quest_id: _,
        entry,
        count,
        required,
    } = ev
    {
        quest_objective_kill(entry, count, required, &mut quest);
    }
}

fn on_objective_item(In(ev): In<SessionEvent>, mut quest: ResMut<QuestGiver>) {
    if let SessionEvent::QuestObjectiveItem { item_id, count } = ev {
        quest_objective_item(item_id, count, &mut quest);
    }
}

fn on_objectives_complete(In(ev): In<SessionEvent>, mut quest: ResMut<QuestGiver>) {
    if let SessionEvent::QuestObjectivesComplete { quest_id } = ev {
        quest_objectives_complete(quest_id, &mut quest);
    }
}

fn on_failed(In(ev): In<SessionEvent>, mut quest: ResMut<QuestGiver>) {
    if let SessionEvent::QuestFailed { quest_id, timed } = ev {
        quest_failed(quest_id, timed, &mut quest);
    }
}

fn on_log_full(In(ev): In<SessionEvent>, mut quest: ResMut<QuestGiver>) {
    if let SessionEvent::QuestLogFull = ev {
        quest_log_full(&mut quest);
    }
}

fn on_giver_invalid(In(ev): In<SessionEvent>, mut quest: ResMut<QuestGiver>) {
    if let SessionEvent::QuestGiverInvalid { reason } = ev {
        quest_giver_invalid(reason, &mut quest);
    }
}

fn on_giver_failed(
    In(ev): In<SessionEvent>,
    mut quest: ResMut<QuestGiver>,
    mut quest_log: ResMut<QuestLog>,
    commands: Res<NetCommands>,
) {
    if let SessionEvent::QuestGiverFailed { quest_id, reason } = ev {
        quest_giver_failed(quest_id, reason, &mut quest, &mut quest_log, &commands);
    }
}

/// An open questgiver panel dies with the socket.
fn on_session_end(In(_): In<SessionEvent>, mut quest: ResMut<QuestGiver>) {
    quest.clear_session();
}

/// `SMSG_QUESTGIVER_STATUS`: one guid's `dialog_status`, for the markers and the minimap dot. A
/// GameObject's answer is dropped, as the reference's resolves the guid by `TYPEMASK_UNIT` and
/// exits; vmangos answers GameObjects (`QuestHandler.cpp:40`). The guid's shape gives the same
/// partition. The reference's second test, `UNIT_NPC_FLAGS & 0x2` on the unit, is not built; the
/// marker query's teardown covers a giver whose flag clears.
fn quest_giver_status(npc: u64, status: u32, quest: &mut QuestGiver) {
    use benilla_protocol::guid;
    if !(guid::is_player(npc) || guid::is_creature_or_pet(npc)) {
        debug!("net: dropping a non-unit questgiver status ({npc:#x} → {status}) — typemask 8");
        quest.refuse_status(npc, status);
        return;
    }
    quest.set_status(npc, status);
}

/// `SMSG_QUESTGIVER_QUEST_LIST`: the greeting panel's quest rows.
fn quest_greeting(list: QuestGiverList, quest: &mut QuestGiver) {
    debug!(
        "net: quest greeting on {:#x} — {} quests",
        list.npc,
        list.quests.len()
    );
    quest.open(list.npc, crate::ui_quest::QuestView::Greeting(list));
}

/// `SMSG_QUESTGIVER_QUEST_DETAILS`: the accept panel.
fn quest_detail(d: QuestDetails, quest: &mut QuestGiver, commands: &NetCommands) {
    debug!("net: quest detail — quest {} on {:#x}", d.quest_id, d.npc);
    // A share landing on an open window is refused by the client with `BUSY`, and the open panel
    // stays; the server's own busy test only knows about other shares.
    if benilla_protocol::guid::is_player(d.npc) && quest.is_open() {
        debug!(
            "net: busy — refusing {:#x}'s shared quest {}",
            d.npc, d.quest_id
        );
        // The reply goes to the inform guid when set, else the source (`0x6d0797`-`0x6d07ba`).
        let _ = commands.0.send(ClientCommand::QuestPushResult {
            sharer: if d.sharer != 0 { d.sharer } else { d.npc },
            quest: d.quest_id,
            msg: QuestShareMsg::BUSY,
        });
        return;
    }
    // The auto-launched byte is the decline latch, not part of the view (`0x58de4f`).
    quest.close_on_cancel = u32::from(d.auto_launched);
    quest.open(d.npc, crate::ui_quest::QuestView::Detail(d));
}

/// `SMSG_QUESTGIVER_REQUEST_ITEMS`: the progress panel.
fn quest_progress(p: QuestRequestItems, quest: &mut QuestGiver) {
    debug!(
        "net: quest progress — quest {} on {:#x} (complete: {})",
        p.quest_id, p.npc, p.is_complete
    );
    // The progress publisher writes `closeOnCancel` into the same latch.
    quest.close_on_cancel = p.close_on_cancel;
    quest.open(p.npc, crate::ui_quest::QuestView::Progress(p));
}

/// `SMSG_QUESTGIVER_OFFER_REWARD`: the reward panel.
fn quest_offer(o: QuestOfferReward, quest: &mut QuestGiver) {
    debug!(
        "net: quest reward offer — quest {} on {:#x}",
        o.quest_id, o.npc
    );
    // The byte after the reward text lands in the latch through DETAILS' publisher `0x58dd50`.
    quest.close_on_cancel = u32::from(o.auto_finish);
    quest.open(o.npc, crate::ui_quest::QuestView::Reward(o));
}

/// `SMSG_QUESTGIVER_QUEST_COMPLETE` (`0x6d1110`): the turn-in's chat lines, then the window
/// closes unless the quest is `chained` (`0x6d10ca`: `nextQuestInChain != 0` keeps it open, with
/// no `QUEST_FINISHED`). The XP, money and items themselves arrive by `UPDATE_OBJECT` and
/// `ITEM_PUSH_RESULT`; 3.3.5a's body has no item list: the fixed rewards print from the template
/// (`0x6d1018`). Deviation: the reference prints and closes from the template's callback, so with
/// the template uncached its window waits; this closes at once.
fn quest_complete(c: QuestComplete, chained: bool, quest: &mut QuestGiver, lines: &mut QuestLines) {
    // The reference plays the `QUESTCOMPLETED` fanfare on this packet.
    quest.completed_fanfare = true;
    debug!(
        "net: quest {} complete — +{} XP, +{} copper",
        c.quest_id, c.xp, c.money
    );
    lines.push(QuestLine::Completed(c.quest_id));
    if c.xp != 0 {
        lines.push(QuestLine::Experience(c.xp));
    }
    // A signed test.
    if (c.money as i32) > 0 {
        lines.push(QuestLine::Money(c.money));
    }
    // Signed tests, after the honor line.
    if c.talents as i32 > 0 {
        lines.push(QuestLine::Talents(c.talents));
    }
    // Deviation: the arena award's `COMBATLOG_ARENAPOINTSAWARD` line goes out as chat type `0x32`
    // (`0x6d0ff4`), which has no `ChatTypeInfo` row in the stock UI, so it is not raised.
    lines.push(QuestLine::RewardItems(c.quest_id));
    if quest.chosen_reward != 0 {
        lines.push(QuestLine::Item(std::mem::take(&mut quest.chosen_reward)));
    }
    if !chained {
        quest.clear();
    }
    // A turn-in can move every giver's marker, so the reference re-asks from here.
    quest.bump_reask();
}

/// `SMSG_QUESTUPDATE_ADD_KILL`: its line is the quest log's (`crate::ui_quest_log`).
fn quest_objective_kill(entry: u32, count: u32, required: u32, quest: &mut QuestGiver) {
    debug!("net: quest kill/use objective {entry:#x} at {count}/{required}");
    quest.bump_reask();
}

/// `SMSG_QUESTUPDATE_ADD_ITEM`, which 12340 registers no handler for; its line comes from the
/// item push.
fn quest_objective_item(item_id: u32, count: u32, quest: &mut QuestGiver) {
    debug!("net: quest item objective {item_id} +{count}");
    quest.bump_reask();
}

/// `SMSG_QUESTUPDATE_COMPLETE` (0x198): its line is the quest log's (`crate::ui_quest_log`).
fn quest_objectives_complete(quest_id: u32, quest: &mut QuestGiver) {
    debug!("net: quest {quest_id} objectives complete");
    // The turn-in `?` can turn gold with no quest-log change, so the reference re-asks here.
    quest.bump_reask();
}

/// `SMSG_QUESTUPDATE_FAILED` (`0x196`) or `_FAILEDTIMER` (`0x197`): its line is the quest log's
/// (`crate::ui_quest_log`).
fn quest_failed(quest_id: u32, timed: bool, quest: &mut QuestGiver) {
    debug!("net: quest {quest_id} failed (timed: {timed})");
    // A failure moves what the givers offer, so the reference re-asks here too.
    quest.bump_reask();
}

/// `SMSG_QUESTLOG_FULL`: the reference's `0x195` arm only raises msg 153, `ERR_QUEST_LOG_FULL`, a
/// red error line, and leaves the panel open.
fn quest_log_full(quest: &mut QuestGiver) {
    debug!("net: quest log full");
    quest.push_message(UiError::key("ERR_QUEST_LOG_FULL"));
}

/// `SMSG_QUESTGIVER_QUEST_INVALID`: a `QuestFailedReason` and no quest id, shown in chat through
/// `questgiver_invalid_key`. Both refusal handlers then close the window with, which zeroes the
/// giver guid and fires `QUEST_FINISHED`.
fn quest_giver_invalid(reason: u32, quest: &mut QuestGiver) {
    debug!("net: questgiver refused to offer the quest (reason {reason})");
    quest.push_message(with_daily_cap(crate::ui_quest::questgiver_invalid_key(
        reason,
    )));
    quest.clear();
}

/// A reason key as its message: the daily cap's line takes the cap, the rest no fill.
fn with_daily_cap(key: &'static str) -> UiError {
    if key == "ERR_QUEST_FAILED_TOO_MANY_DAILY_QUESTS_I" {
        UiError::args(
            key,
            vec![crate::ui_action::FillArg::D(
                crate::ui_quest::DAILY_QUEST_CAP,
            )],
        )
    } else {
        UiError::key(key)
    }
}

/// `SMSG_QUESTGIVER_QUEST_FAILED`: the line's `%s` is the quest's title, from the open panel or the
/// template cache. A full bag adds msg 0, `ERR_INV_FULL`, on the red surface; the default line is
/// silent for a quest a timer expired (`0x6cdb03`); and the window closes as on an invalid quest.
fn quest_giver_failed(
    quest_id: u32,
    reason: u32,
    quest: &mut QuestGiver,
    quest_log: &mut QuestLog,
    net_commands: &NetCommands,
) {
    debug!("net: quest {quest_id} accept failed (reason {reason})");
    let title = quest
        .view_title(quest_id)
        .or_else(|| {
            quest_log
                .template(quest_id, net_commands)
                .map(|t| t.title.clone())
        })
        .unwrap_or_default();
    let key = crate::ui_quest::questgiver_failed_key(reason);
    match key {
        "ERR_QUEST_FAILED_TOO_MANY_DAILY_QUESTS_I" => quest.push_message(with_daily_cap(key)),
        "ERR_QUEST_FAILED_S" if quest_log.is_expired(quest_id) => {}
        _ => quest.push_message(UiError::s(key, title)),
    }
    if key == "ERR_QUEST_FAILED_BAG_FULL_S" {
        quest.push_message(UiError::key("ERR_INV_FULL"));
    }
    quest.clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Typemask 8 is `TYPEMASK_UNIT`, which a creature (`0x9`) and a player (`0x19`) both carry.
    #[test]
    fn a_gameobjects_dialog_status_is_dropped_and_a_creatures_is_not() {
        use benilla_protocol::messages::dialog_status;

        // A GameObject (Goldshire's Wanted Poster), a transport (an elevator), a creature and a
        // player.
        const POSTER: u64 = 0xf110_0000_0044_68db;
        const ELEVATOR: u64 = 0xf120_0000_0384_1092;
        const CREATURE: u64 = 0xf130_0000_0060_0abc;
        const PLAYER: u64 = 0x0000_0000_0000_0007;

        let mut quest = QuestGiver::default();
        for (guid, status) in [
            (POSTER, dialog_status::AVAILABLE),
            (ELEVATOR, dialog_status::AVAILABLE),
            (CREATURE, dialog_status::AVAILABLE),
            (PLAYER, dialog_status::REWARD2),
        ] {
            quest_giver_status(guid, status, &mut quest);
        }
        assert_eq!(
            quest.status(POSTER),
            None,
            "a GameObject GUID misses typemask bit 3 and the handler exits"
        );
        assert_eq!(quest.status(ELEVATOR), None, "…and so does a transport GO");
        assert_eq!(
            quest.status(CREATURE),
            Some(dialog_status::AVAILABLE),
            "the control: a creature's answer is what this packet is FOR"
        );
        assert_eq!(
            quest.status(PLAYER),
            Some(dialog_status::REWARD2),
            "and a player passes the same typemask, as it does in the reference"
        );
    }

    /// The turn-in's awards (`0x6d0ed0`-`0x6d0fd0`): a tenth of the wire's honor to
    /// `HONOR_GAINED`, the arena points to `ARENA_POINTS_GAINED`, and nothing without either.
    #[test]
    fn a_turn_in_sends_its_honor_and_arena_awards_to_the_combat_text() {
        use benilla_ui::script::ScriptValue;
        use bevy::ecs::system::RunSystemOnce;
        let sent = |c: QuestComplete| {
            let mut app = App::new();
            app.init_resource::<crate::ui_chat::ChatLog>()
                .add_message::<crate::ui_unit::CombatTextEvent>();
            app.world_mut()
                .run_system_once(
                    move |mut chat: ResMut<crate::ui_chat::ChatLog>,
                          mut center: MessageWriter<crate::ui_unit::CombatTextEvent>| {
                        turn_in_awards(&c, &mut chat, &mut center)
                    },
                )
                .unwrap();
            app.world_mut()
                .resource_mut::<Messages<crate::ui_unit::CombatTextEvent>>()
                .drain()
                .map(|e| (e.message_type, e.args))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            sent(QuestComplete {
                quest_id: 7,
                honor: 1_250,
                arena_points: 15,
                ..Default::default()
            }),
            [
                ("HONOR_GAINED", vec![ScriptValue::Int(125)]),
                ("ARENA_POINTS_GAINED", vec![ScriptValue::Int(15)]),
            ]
        );
        assert!(sent(QuestComplete {
            quest_id: 7,
            ..Default::default()
        })
        .is_empty());
    }

    /// `0x6d0e10`'s order: the completion, the XP, the money, the template's reward items, then
    /// the chosen reward, which the turn-in spends.
    #[test]
    fn a_turn_in_raises_its_lines_in_the_reference_order() {
        let mut giver = QuestGiver {
            chosen_reward: 2_047,
            ..Default::default()
        };
        let mut lines = QuestLines::default();
        quest_complete(
            QuestComplete {
                quest_id: 7,
                xp: 450,
                money: 1_025,
                talents: 1,
                arena_points: 15,
                ..Default::default()
            },
            false,
            &mut giver,
            &mut lines,
        );
        assert_eq!(
            lines.queued(),
            [
                QuestLine::Completed(7),
                QuestLine::Experience(450),
                QuestLine::Money(1_025),
                QuestLine::Talents(1),
                QuestLine::RewardItems(7),
                QuestLine::Item(2_047),
            ]
        );
        assert_eq!(giver.chosen_reward, 0, "the turn-in zeroes the latch");

        // No XP (a capped level) and no money: the completion and the template's items.
        let mut lines = QuestLines::default();
        quest_complete(
            QuestComplete {
                quest_id: 7,
                ..Default::default()
            },
            false,
            &mut giver,
            &mut lines,
        );
        assert_eq!(
            lines.queued(),
            [QuestLine::Completed(7), QuestLine::RewardItems(7)]
        );
    }

    /// `0x6d10ca`: a chained turn-in leaves the window to the next quest's panel.
    #[test]
    fn a_chained_turn_in_keeps_the_window_and_a_last_one_closes_it() {
        let turn_in = |chained| {
            let mut giver = open_detail(7);
            let mut lines = QuestLines::default();
            quest_complete(
                QuestComplete {
                    quest_id: 7,
                    ..Default::default()
                },
                chained,
                &mut giver,
                &mut lines,
            );
            giver.is_open()
        };
        assert!(turn_in(true));
        assert!(!turn_in(false));
    }

    fn open_detail(quest_id: u32) -> QuestGiver {
        let mut giver = QuestGiver::default();
        giver.open(
            0x4000_0000_0000_0BAD,
            crate::ui_quest::QuestView::Detail(QuestDetails {
                npc: 0x4000_0000_0000_0BAD,
                quest_id,
                title: "A Threat Within".into(),
                details: String::new(),
                objectives: String::new(),
                auto_launched: false,
                choices: vec![],
                rewards: vec![],
                ..Default::default()
            }),
        );
        giver
    }

    /// Reason 13: the quest is already in the log.
    #[test]
    fn an_already_on_refusal_speaks_and_closes_the_panel() {
        let mut giver = open_detail(373);
        quest_giver_invalid(13, &mut giver);
        let msgs = giver.take_messages();
        assert_eq!(msgs.len(), 1);
        assert_eq!(
            benilla_ui::messages::kind_of(msgs[0].key),
            benilla_ui::messages::MsgKind::Chat
        );
        assert_eq!(msgs[0], UiError::key("ERR_QUEST_ALREADY_ON"));
        assert!(!giver.is_open(), "the ref closes the window on a refusal");
    }

    #[test]
    fn a_full_bag_refusal_names_the_quest_and_adds_the_inventory_line() {
        let mut giver = open_detail(373);
        let mut log = QuestLog::default();
        let (tx, _rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        quest_giver_failed(373, 4, &mut giver, &mut log, &commands);
        let msgs = giver.take_messages();
        assert_eq!(msgs.len(), 2);
        assert_eq!(
            benilla_ui::messages::kind_of(msgs[0].key),
            benilla_ui::messages::MsgKind::Chat
        );
        assert_eq!(
            msgs[0],
            UiError::s("ERR_QUEST_FAILED_BAG_FULL_S", "A Threat Within")
        );
        assert_eq!(
            benilla_ui::messages::kind_of(msgs[1].key),
            benilla_ui::messages::MsgKind::Error
        );
        assert_eq!(msgs[1], UiError::key("ERR_INV_FULL"));
        assert!(!giver.is_open());
    }

    #[test]
    fn a_full_log_takes_the_red_line_and_leaves_the_panel_alone() {
        let mut giver = open_detail(373);
        quest_log_full(&mut giver);
        let msgs = giver.take_messages();
        assert_eq!(msgs.len(), 1);
        assert_eq!(
            benilla_ui::messages::kind_of(msgs[0].key),
            benilla_ui::messages::MsgKind::Error
        );
        assert_eq!(msgs[0], UiError::key("ERR_QUEST_LOG_FULL"));
        assert!(
            giver.is_open(),
            "the ref's 0x195 arm does not close the panel"
        );
    }

    #[test]
    fn a_refusal_for_another_quest_does_not_borrow_the_open_title() {
        let mut giver = open_detail(373);
        let mut log = QuestLog::default();
        let (tx, _rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        quest_giver_failed(999, 17, &mut giver, &mut log, &commands);
        let msgs = giver.take_messages();
        assert_eq!(msgs[0].key, "ERR_QUEST_FAILED_MAX_COUNT_S");
        assert_eq!(msgs[0].arg_s(), Some(""));
    }

    // ── The share's BUSY refusal ─────────────────────────────────────────────────

    fn detail(npc: u64, quest_id: u32) -> QuestDetails {
        QuestDetails {
            npc,
            quest_id,
            title: "A Threat Within".into(),
            details: String::new(),
            objectives: String::new(),
            auto_launched: false,
            choices: Vec::new(),
            rewards: Vec::new(),
            ..Default::default()
        }
    }

    #[test]
    fn a_share_on_top_of_an_open_window_is_refused_as_busy() {
        const SHARER: u64 = 0x0000_0000_0000_002A;
        const NPC: u64 = 0xF130_0000_0000_0007;
        let (tx, rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        let mut quest = QuestGiver::default();

        quest_detail(detail(NPC, 100), &mut quest, &commands);
        assert_eq!(quest.npc, Some(NPC));
        assert!(rx.try_iter().next().is_none(), "opening sends nothing");

        quest_detail(detail(SHARER, 200), &mut quest, &commands);
        assert_eq!(quest.npc, Some(NPC), "the open window survives the refusal");
        let sent: Vec<_> = rx.try_iter().collect();
        assert!(
            matches!(
                sent.as_slice(),
                [ClientCommand::QuestPushResult {
                    sharer: SHARER,
                    quest: 200,
                    msg: QuestShareMsg::BUSY,
                }]
            ),
            "the sharer is told we are busy: {sent:?}"
        );
    }

    #[test]
    fn a_share_with_no_window_open_just_opens() {
        const SHARER: u64 = 0x0000_0000_0000_002A;
        let (tx, rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        let mut quest = QuestGiver::default();

        quest_detail(detail(SHARER, 200), &mut quest, &commands);
        assert_eq!(quest.npc, Some(SHARER));
        assert!(rx.try_iter().next().is_none(), "no verdict, no refusal");
    }

    #[test]
    fn the_latch_is_written_by_every_panel_packet_and_cleared_on_world_enter() {
        const NPC: u64 = 0xF130_0000_0000_0007;
        let (tx, _rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        let mut quest = QuestGiver::default();

        let mut d = detail(NPC, 100);
        d.auto_launched = true;
        quest_detail(d, &mut quest, &commands);
        assert_eq!(quest.close_on_cancel, 1, "DETAILS writes it");

        quest_progress(progress(NPC, 100, 1), &mut quest);
        assert_eq!(
            quest.close_on_cancel, 1,
            "REQUEST_ITEMS' closeOnCancel writes it"
        );
        quest_progress(progress(NPC, 100, 0), &mut quest);
        assert_eq!(quest.close_on_cancel, 0, "…including a zero");

        quest_offer(offer(NPC, 100, 1), &mut quest);
        assert_eq!(quest.close_on_cancel, 1, "OFFER_REWARD writes it");

        world_enter_reset(&mut quest);
        assert_eq!(quest.close_on_cancel, 0, "the world-enter reset zeroes it");
    }

    /// Every panel publish clears the already-acted latch.
    #[test]
    fn a_panel_packet_clears_the_acted_latch() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        let mut quest = QuestGiver {
            acted: true,
            ..Default::default()
        };
        quest_detail(detail(0xF130_0000_0000_0007, 100), &mut quest, &commands);
        assert!(!quest.acted);
    }

    fn progress(npc: u64, quest_id: u32, close_on_cancel: u32) -> QuestRequestItems {
        QuestRequestItems {
            npc,
            quest_id,
            title: "A Threat Within".into(),
            request_text: String::new(),
            emote_delay: 0,
            emote: 0,
            close_on_cancel,
            quest_flags: 0,
            suggested_players: 0,
            required_money: 0,
            required_items: Vec::new(),
            is_complete: false,
        }
    }

    /// Every panel reader plays its emotes on the NPC; a player's shared details play none.
    #[test]
    fn each_panel_packet_queues_its_emotes_on_the_npc() {
        const NPC: u64 = 0xF130_0000_0000_0007;
        const SHARER: u64 = 0x0000_0000_0000_002A;
        let ask = |ev: SessionEvent| panel_emotes(&ev).map(|a| (a.guid, a.emotes));
        let list = QuestGiverList {
            npc: NPC,
            emote_delay: 30,
            emote: 1,
            ..Default::default()
        };
        assert_eq!(
            ask(SessionEvent::QuestGreeting(list)),
            Some((NPC, vec![(30, 1)]))
        );
        let mut d = detail(NPC, 100);
        d.emotes = vec![(0, 1), (60, 2)];
        assert_eq!(
            ask(SessionEvent::QuestDetail(d.clone())),
            Some((NPC, vec![(0, 1), (60, 2)]))
        );
        d.npc = SHARER;
        assert_eq!(
            ask(SessionEvent::QuestDetail(d)),
            None,
            "a sharer plays nothing"
        );
        let mut p = progress(NPC, 100, 0);
        (p.emote_delay, p.emote) = (10, 6);
        assert_eq!(
            ask(SessionEvent::QuestProgress(p)),
            Some((NPC, vec![(10, 6)]))
        );
        let mut o = offer(NPC, 100, 0);
        o.emotes = vec![(0, 4)];
        assert_eq!(ask(SessionEvent::QuestOffer(o)), Some((NPC, vec![(0, 4)])));
    }

    fn offer(npc: u64, quest_id: u32, auto_finish: u32) -> QuestOfferReward {
        QuestOfferReward {
            npc,
            quest_id,
            title: "A Threat Within".into(),
            offer_text: String::new(),
            auto_finish: auto_finish != 0,
            ..Default::default()
        }
    }
}
