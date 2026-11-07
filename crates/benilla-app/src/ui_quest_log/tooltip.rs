//! The quest tooltip's feed: a `quest:` link names any quest, in the log or not, so its record comes
//! off the template cache, and the leaderboard's lines take their no-progress keys (`0x5e1460`
//! with no log entry, as `0x622960` calls it). Each ask stays open until every name it reads lands.

use benilla_protocol::messages::QuestTemplate;
use benilla_ui::script::{QuestTooltipView, UiScript};
use benilla_ui::strings::{fill, Arg};
use bevy::prelude::*;

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

/// The no-progress leaderboard's lines, or `None` while a name is in flight. An id the server does
/// not know reads as the reference's blank.
fn requirement_lines(
    t: &QuestTemplate,
    n: &Names<'_>,
    get: &dyn Fn(&str) -> Option<String>,
) -> Option<Vec<String>> {
    let mut lines = Vec::new();
    if !t.end_text.is_empty() {
        lines.push(t.end_text.clone());
    }
    let line = |key: &str, name: &str, count: u32| {
        get(key).map_or_else(String::new, |f| {
            fill(&f, &[Arg::S(name), Arg::D(i64::from(count))])
        })
    };
    let mut waiting = false;
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
        let name = if is_go {
            n.gos
                .and_then(|g| g.name_by_entry(o.creature_or_go & !GO_OBJECTIVE_BIT, n.commands))
        } else {
            n.names.resolve_creature(o.creature_or_go, 0, n.commands)
        };
        waiting |= name.is_none();
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
        waiting |= template.is_none() && !n.items.template_answered_unknown(item);
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
            .and_then(|f| f.catalog().faction_name(t.rep_objective_faction))
        {
            let rank = standing_rank(t.rep_objective_value as i32);
            let label = get(&format!("FACTION_STANDING_LABEL{rank}")).unwrap_or_default();
            lines.push(
                get("QUEST_FACTION_NEEDED_NOPROGRESS")
                    .map_or_else(String::new, |f| fill(&f, &[Arg::S(name), Arg::S(&label)])),
            );
        }
    }
    (!waiting).then_some(lines)
}

/// Answers the open asks; runs after the template handler, so a landed record shows the same frame.
pub(super) fn feed_quest_tooltips(
    script: Option<NonSendMut<UiScript>>,
    log: Res<QuestLog>,
    names: Res<NameCache>,
    items: Res<Items>,
    gos: Option<Res<GameObjectTemplates>>,
    factions: Option<Res<crate::target::Factions>>,
    commands: Res<NetCommands>,
    states: Res<crate::world_state::WorldStates>,
    self_q: Query<(&ObjectStore, &Guid), With<SelfPlayer>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let asks = script.quest_tooltip_asks();
    if asks.is_empty() {
        return;
    }
    let player = crate::npc_text::player_identity(&self_q, &names, &commands);
    let macros = crate::npc_text::MacroContext {
        subject: player.as_ref(),
        states: &states,
        achievement: None,
    };
    let caches = Names {
        names: &names,
        items: &items,
        gos: gos.as_deref(),
        factions: factions.as_deref(),
        commands: &commands,
    };
    for quest in asks {
        if log.template_answered_unknown(quest) {
            script.drop_quest_tooltip_ask(quest);
            continue;
        }
        let Some(t) = log.template(quest, &commands) else {
            continue;
        };
        let view = {
            let get = |key: &str| {
                script
                    .lua()
                    .globals()
                    .get::<String>(key)
                    .ok()
                    .filter(|t| !t.is_empty())
            };
            requirement_lines(t, &caches, &get).map(|requirements| QuestTooltipView {
                title: t.title.clone(),
                objectives_text: crate::npc_text::substitute(&t.objectives_text, &macros),
                requirements,
            })
        };
        if let Some(view) = view {
            script.set_quest_tooltip(quest, view);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::standing_rank;

    #[test]
    fn a_required_amount_reads_its_standing() {
        assert_eq!(standing_rank(42000), 8);
        assert_eq!(standing_rank(9000), 6);
        assert_eq!(standing_rank(0), 4);
        assert_eq!(standing_rank(-1), 3);
        assert_eq!(standing_rank(-42000), 1);
    }
}
