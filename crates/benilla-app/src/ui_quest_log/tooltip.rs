//! The quest tooltip's reads at the call: a `quest:` link names any quest, in the log or not, so its record comes off the template cache, and the leaderboard's lines take their no-progress keys (`0x5e1460` with no log entry, as `0x622960` calls it). Each record the build missed builds the tooltip again when its answer lands (`0x61de20`).

use benilla_protocol::messages::QuestTemplate;
use benilla_protocol::{SessionEvent, SessionEventKind};
use benilla_ui::script::{QuestRecord, QuestTooltipRead, QuestTooltipView};
use benilla_ui::strings::{fill, Arg};
use bevy::prelude::*;

use crate::net::NetHandlerApp;
use crate::ui_script::live::UiEvents;

use super::{QuestLog, GO_OBJECTIVE_BIT, NAME_PLACEHOLDER};
use crate::go_templates::GameObjectTemplates;
use crate::items::Items;
use crate::names::NameCache;
use crate::net::{Guid, NetCommands, ObjectStore, SelfPlayer};

/// The standing a required amount reaches (`0x5e1460`), `FACTION_STANDING_LABEL`'s index.
fn standing_rank(amount: i32) -> u32 {
    match amount {
        42000.. => 8,
        21000.. => 7,
        9000.. => 6,
        3000.. => 5,
        0.. => 4,
        -3000.. => 3,
        -6000.. => 2,
        _ => 1,
    }
}

/// The caches the lines read.
struct Names<'a> {
    names: &'a NameCache,
    items: &'a Items,
    gos: Option<&'a GameObjectTemplates>,
    factions: Option<&'a crate::target::Factions>,
    commands: &'a NetCommands,
}

/// The no-progress leaderboard's lines, and the records still in flight, each read as the placeholder. An id the server does not know reads as the reference's blank.
fn requirement_lines(
    t: &QuestTemplate,
    n: &Names<'_>,
    get: &dyn Fn(&str) -> Option<String>,
) -> (Vec<String>, Vec<QuestRecord>) {
    let mut lines = Vec::new();
    if !t.end_text.is_empty() {
        lines.push(t.end_text.clone());
    }
    let line = |key: &str, name: &str, count: u32| {
        get(key).map_or_else(String::new, |f| {
            fill(&f, &[Arg::S(name), Arg::D(i64::from(count))])
        })
    };
    let mut waits = Vec::new();
    for o in t.objectives.iter().filter(|o| o.creature_or_go != 0) {
        let is_go = o.creature_or_go & GO_OBJECTIVE_BIT != 0;
        if !o.text.is_empty() {
            lines.push(line(
                "QUEST_OBJECTS_FOUND_NOPROGRESS",
                &o.text,
                o.required_count,
            ));
            continue;
        }
        let entry = o.creature_or_go & !GO_OBJECTIVE_BIT;
        let name = if is_go {
            n.gos.and_then(|g| g.name_by_entry(entry, n.commands))
        } else {
            n.names.resolve_creature(entry, 0, n.commands)
        };
        if name.is_none() {
            waits.push(if is_go {
                QuestRecord::GameObject(entry)
            } else {
                QuestRecord::Creature(entry)
            });
        }
        let key = if is_go {
            "QUEST_OBJECTS_FOUND_NOPROGRESS"
        } else {
            "QUEST_MONSTERS_KILLED_NOPROGRESS"
        };
        lines.push(line(
            key,
            name.unwrap_or(NAME_PLACEHOLDER),
            o.required_count,
        ));
    }
    for &(item, count) in t.required_items.iter() {
        if item as i32 <= 0 || (item == t.src_item_id && count < 2) {
            continue;
        }
        let template = n.items.template(item, 0, n.commands);
        if template.is_none() && !n.items.template_answered_unknown(item) {
            waits.push(QuestRecord::Item(item));
        }
        lines.push(line(
            "QUEST_ITEMS_NEEDED_NOPROGRESS",
            template.map_or(NAME_PLACEHOLDER, |i| i.name.as_str()),
            count,
        ));
    }
    if t.players_slain != 0 {
        lines.push(
            get("QUEST_PLAYERS_KILLED_NOPROGRESS").map_or_else(String::new, |f| {
                fill(&f, &[Arg::D(i64::from(t.players_slain))])
            }),
        );
    } else if t.rep_objective_faction != 0 {
        if let Some(name) = n
            .factions
            .and_then(|f| f.catalog().faction_name_or_empty(t.rep_objective_faction))
        {
            let rank = standing_rank(t.rep_objective_value as i32);
            let label = get(&format!("FACTION_STANDING_LABEL{rank}")).unwrap_or_default();
            lines.push(
                get("QUEST_FACTION_NEEDED_NOPROGRESS")
                    .map_or_else(String::new, |f| fill(&f, &[Arg::S(name), Arg::S(&label)])),
            );
        }
    }
    (lines, waits)
}

/// What the quest tooltip reads at the call: the caches and the player its text names.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct QuestTooltipLive<'w, 's> {
    log: Option<Res<'w, QuestLog>>,
    names: Option<Res<'w, NameCache>>,
    items: Option<Res<'w, Items>>,
    gos: Option<Res<'w, GameObjectTemplates>>,
    factions: Option<Res<'w, crate::target::Factions>>,
    commands: Option<Res<'w, NetCommands>>,
    states: Option<Res<'w, crate::world_state::WorldStates>>,
    self_q: Query<'w, 's, (&'static ObjectStore, &'static Guid), With<SelfPlayer>>,
}

impl QuestTooltipLive<'_, '_> {
    /// The tooltip's build of `quest` at the call (`0x622960`); `None` without the caches.
    pub(crate) fn read(
        &self,
        quest: u32,
        get: &dyn Fn(&str) -> Option<String>,
    ) -> Option<QuestTooltipRead> {
        let (log, names, items, commands, states) = (
            self.log.as_deref()?,
            self.names.as_deref()?,
            self.items.as_deref()?,
            self.commands.as_deref()?,
            self.states.as_deref()?,
        );
        let Some(t) = log.template(quest, commands) else {
            let waits = (!log.template_answered_unknown(quest))
                .then_some(QuestRecord::Quest(quest))
                .into_iter()
                .collect();
            return Some(QuestTooltipRead { view: None, waits });
        };
        let player = crate::npc_text::player_identity(&self.self_q, names, commands);
        let macros = crate::npc_text::MacroContext {
            subject: player.as_ref(),
            states,
            achievement: None,
        };
        let caches = Names {
            names,
            items,
            gos: self.gos.as_deref(),
            factions: self.factions.as_deref(),
            commands,
        };
        let (requirements, waits) = requirement_lines(t, &caches, get);
        Some(QuestTooltipRead {
            view: Some(QuestTooltipView {
                title: t.title.clone(),
                objectives_text: crate::npc_text::substitute(&t.objectives_text, &macros),
                requirements,
            }),
            waits,
        })
    }
}

/// Each quest, creature or GameObject record answer runs the quest tooltips' callbacks for it once the cache holds it (`0x61de20`): registered after the caches' own handlers, a found record builds every tooltip whose build missed it; an item record's run on the item cache's list.
pub(super) fn register(app: &mut App) {
    use SessionEventKind as K;
    app.net_handler(K::QuestTemplate, on_record)
        .net_handler(K::CreatureName, on_record)
        .net_handler(K::GameObjectInfo, on_record);
}

fn on_record(In(ev): In<SessionEvent>, mut ui: ResMut<UiEvents>) {
    let record = match ev {
        SessionEvent::QuestTemplate(t) => QuestRecord::Quest(t.quest_id),
        SessionEvent::CreatureName {
            entry,
            name: Some(_),
            ..
        } => QuestRecord::Creature(entry),
        SessionEvent::GameObjectInfo { entry, .. } => QuestRecord::GameObject(entry),
        _ => return,
    };
    ui.write(move |script| script.quest_record_landed(record));
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_formats::FactionCatalog;
    use benilla_ui::script::UiScript;
    use std::collections::HashMap;

    /// The leaderboard lines for a quest needing `faction`, over a catalog where 6 is named and 7
    /// has a row with an empty name.
    fn rep_lines(faction: u32) -> Vec<String> {
        let factions = crate::target::Factions::from_catalog(
            FactionCatalog::from_rows(HashMap::new(), HashMap::new())
                .with_faction(6, Some("Darnassus"))
                .with_faction(7, None),
        );
        let (tx, _rx) = crossbeam_channel::unbounded();
        let (names, items, commands) = (NameCache::default(), Items::default(), NetCommands(tx));
        let n = Names {
            names: &names,
            items: &items,
            gos: None,
            factions: Some(&factions),
            commands: &commands,
        };
        let t = QuestTemplate {
            rep_objective_faction: faction,
            rep_objective_value: 3000,
            ..Default::default()
        };
        let get = |key: &str| match key {
            "QUEST_FACTION_NEEDED_NOPROGRESS" => Some("%s: %s".to_string()),
            "FACTION_STANDING_LABEL5" => Some("Friendly".to_string()),
            _ => None,
        };
        let (lines, waits) = requirement_lines(&t, &n, &get);
        assert!(waits.is_empty(), "nothing in flight");
        lines
    }

    /// The line needs only the faction row and formats its name as stored (`0x5e1ecc`), empty
    /// included; an id with no row gives no line (`0x5e1d8a`).
    #[test]
    fn a_reputation_line_needs_the_row_and_keeps_an_empty_name() {
        assert_eq!(rep_lines(6), ["Darnassus: Friendly"]);
        assert_eq!(rep_lines(7), [": Friendly"]);
        assert!(rep_lines(8).is_empty());
    }

    /// A minimal app with the record handlers, the caches holding quest 176 (Hogger, creature 448 in flight), and a VM with a quest tooltip `T`.
    fn tooltip_app() -> App {
        let mut app = App::new();
        let (tx, _rx) = crossbeam_channel::unbounded();
        app.add_plugins(MinimalPlugins)
            .init_resource::<UiEvents>()
            .init_resource::<NameCache>()
            .init_resource::<Items>()
            .init_resource::<crate::world_state::WorldStates>()
            .insert_resource(NetCommands(tx));
        register(&mut app);
        let mut log = QuestLog::default();
        let mut t = QuestTemplate {
            quest_id: 176,
            title: "Wanted: Hogger".into(),
            objectives_text: "Bring the paw.".into(),
            ..Default::default()
        };
        t.objectives[0].creature_or_go = 448;
        t.objectives[0].required_count = 1;
        log.insert_template(t);
        app.insert_resource(log);
        let script = UiScript::new().unwrap();
        script
            .run(
                r#"
                QUEST_TOOLTIP_REQUIREMENTS = "Requirements:"
                QUEST_MONSTERS_KILLED_NOPROGRESS = "%s slain: %d"
                fired = 0
                T = CreateFrame("GameTooltip", "T")
                CreateFrame("Frame", "Owner")
                T:SetOwner(Owner, "ANCHOR_PRESERVE")
                T:SetScript("OnTooltipSetQuest", function() fired = fired + 1 end)
                "#,
            )
            .unwrap();
        app.insert_non_send_resource(script);
        app
    }

    fn line(app: &App, n: u32) -> String {
        app.world()
            .non_send_resource::<UiScript>()
            .eval(&format!("return TTextLeft{n}:GetText()"))
            .unwrap()
    }

    fn creature_answer(entry: u32) -> SessionEvent {
        SessionEvent::CreatureName {
            entry,
            name: Some("Hogger".into()),
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
        }
    }

    /// `SetHyperlink` reads the caches at the call (`0x622960`): the name in flight shows as the placeholder and is asked; its answer builds the tooltip again with the name (`0x61de20`), and another creature's answer builds nothing.
    #[test]
    fn the_tooltip_reads_the_caches_at_the_call_and_a_name_answer_rebuilds_it() {
        use crate::net::handlers::dispatch;
        let mut app = tooltip_app();
        crate::ui_script::live::with_live_world(app.world_mut(), |s| {
            s.run(r#"T:SetHyperlink("quest:176:1")"#).unwrap();
        });
        assert_eq!(line(&app, 1), "Wanted: Hogger");
        assert_eq!(line(&app, 6), " - ... slain: 1");
        let fired = |app: &App| {
            app.world()
                .non_send_resource::<UiScript>()
                .eval::<i64>("return fired")
                .unwrap()
        };
        assert_eq!(fired(&app), 1);

        // Another creature's answer: no rebuild.
        dispatch(app.world_mut(), vec![creature_answer(99)]);
        assert_eq!(fired(&app), 1);

        // The cache holds the name before the callback runs, as the cache's handler comes first.
        app.world_mut().resource_mut::<NameCache>().insert_creature(
            448,
            Some(crate::names::CreatureRecord {
                name: "Hogger".into(),
                ..Default::default()
            }),
        );
        dispatch(app.world_mut(), vec![creature_answer(448)]);
        assert_eq!(fired(&app), 2);
        assert_eq!(line(&app, 6), " - Hogger slain: 1");
    }

    /// A template the server does not know waits on nothing; one in flight waits on its answer.
    #[test]
    fn a_template_miss_reads_no_view_and_waits_only_while_in_flight() {
        let mut app = tooltip_app();
        let mut state =
            bevy::ecs::system::SystemState::<QuestTooltipLive<'_, '_>>::new(app.world_mut());
        let get = |_: &str| None;
        let read = state.get(app.world()).read(500, &get).unwrap();
        assert_eq!(read.view, None);
        assert_eq!(read.waits, [QuestRecord::Quest(500)]);
        app.world_mut()
            .resource_mut::<QuestLog>()
            .templates
            .insert(500, None);
        let read = state.get(app.world()).read(500, &get).unwrap();
        assert_eq!(read.view, None);
        assert!(read.waits.is_empty());
    }

    #[test]
    fn a_required_amount_reads_its_standing() {
        assert_eq!(standing_rank(42000), 8);
        assert_eq!(standing_rank(9000), 6);
        assert_eq!(standing_rank(0), 4);
        assert_eq!(standing_rank(-1), 3);
        assert_eq!(standing_rank(-42000), 1);
    }
}
