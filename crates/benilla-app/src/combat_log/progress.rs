//! The progression lines the combat-log module also handles: the XP number and line, the
//! exploration toast and jingle, and the level-up block.

use bevy::prelude::*;

use benilla_protocol::messages::{ExplorationXp, LevelUpInfo, XpGain};

use crate::combat_text::CombatTextSpawn;
use crate::net::{GuidIndex, ObjectStore, SelfGuid};
use crate::ui_chat::{ChatLog, XpLine, REST_STATE_KEYS};
use crate::ui_unit::RestStates;

/// `SMSG_LOG_XPGAIN`: `"XP: %d"` over self, category 4, the one emitter that skips Gate A, for kill
/// and quest XP alike (`0x50b337`), then the chat line. A loss on our own player shows the line
/// alone (`0x50c7bc`).
pub(super) fn xp_gain(
    x: XpGain,
    me: Option<Entity>,
    line: XpLine,
    text: &mut MessageWriter<CombatTextSpawn>,
    chat_log: &mut ChatLog,
) {
    if let (Some(me), false) = (me, matches!(line, XpLine::Loss { .. })) {
        text.write(CombatTextSpawn {
            anchor: me,
            text: format!("XP: {}", x.total),
            category: 4,
            color: None, // row 4's own purple: XP skips the damage colour law
        });
    }
    chat_log.push_xp_gain(line);
}

/// The XP line's form (`0x50c7b0`, `0x50b300`). Our own guid is the loss line. RAF quest XP (types
/// 1 and 6) takes the quest line. A kill on a held unit, with our player streamed, reads the rest
/// state, or 6 under RAF (`0x50b36a`); a state whose factor is neither 0 nor 1 takes its key.
/// Anything else is the plain line, named only for a held unit (`0x50b1f1`).
pub(super) fn xp_line(
    x: &XpGain,
    self_guid: Option<u64>,
    me: Option<&ObjectStore>,
    held_unit: impl Fn(u64) -> bool,
    rest_states: Option<&RestStates>,
) -> XpLine {
    if x.victim != 0 && Some(x.victim) == self_guid {
        return XpLine::Loss { total: x.total };
    }
    let held = x.victim != 0 && held_unit(x.victim);
    let state = |id: u8| rest_states.and_then(|r| r.get(id));
    if !held || x.xp_type != 0 {
        if x.refer_a_friend && matches!(x.xp_type, 1 | 6) {
            if let Some((name, _)) = state(6) {
                return XpLine::ReferAFriendQuest {
                    total: x.total,
                    state: name.to_string(),
                };
            }
        }
    } else if let Some(store) = me {
        let id = if x.refer_a_friend {
            6
        } else {
            store.0.player_rest_state().unwrap_or(0)
        };
        let rested = state(id)
            .filter(|(_, factor)| *factor != 0.0 && *factor != 1.0)
            .zip(REST_STATE_KEYS.get(usize::from(id)).copied().flatten());
        if let Some(((name, _), key)) = rested {
            return XpLine::Rested {
                victim: x.victim,
                key,
                state: name.to_string(),
                total: x.total,
                base: x.base,
                group_rate: x.group_rate,
            };
        }
    }
    XpLine::Plain {
        victim: if held { x.victim } else { 0 },
        total: x.total,
        group_rate: x.group_rate,
    }
}

/// `SMSG_EXPLORATION_EXPERIENCE` (`0x6e36ea`): an area with no `AreaTable.dbc` row shows and plays
/// nothing (`0x6e3712`). Otherwise the ERR_ZONE_EXPLORED toast, then, when xp > 0 and
/// `PLAYER_FLAGS` lacks no-XP-gain (`0x6e3748`), the ERR_ZONE_EXPLORED_XP chat line, via
/// [`ChatLog::push_exploration`]; then the race's discovery jingle (`ChrRaces.dbc` col 3 to
/// SoundEntries, `0x6e379b`), through the same 2D path as `SMSG_PLAY_SOUND`. No floating text:
/// the XP arrives separately as `SMSG_LOG_XPGAIN`.
pub(super) fn exploration_xp(
    x: ExplorationXp,
    area_table: Option<&crate::area::AreaTableRes>,
    sounds: Option<&crate::sound::ExplorationSounds>,
    index: &GuidIndex,
    self_guid: &SelfGuid,
    stores: &Query<&ObjectStore>,
    sound_out: &mut MessageWriter<crate::net::ServerSoundMessage>,
    chat_log: &mut ChatLog,
) {
    let Some(name) = area_table.and_then(|t| t.0.name(x.area_id)) else {
        warn!(
            "exploration: discovered area {} has no AreaTable row — no message, no sound",
            x.area_id
        );
        return;
    };
    let me = self_guid
        .0
        .as_ref()
        .and_then(|g| index.0.get(g))
        .and_then(|&e| stores.get(e).ok());
    let race = me.and_then(|s| s.0.unit_race());
    let kit = race.and_then(|r| sounds.and_then(|s| s.0.kit(u32::from(r))));
    let xp = exploration_line_xp(x.xp, me.map(|s| s.0.player_flags()));
    info!(
        "exploration: discovered {name} (area {}, {} xp, line xp {xp}, jingle kit {})",
        x.area_id,
        x.xp,
        kit.unwrap_or(0)
    );
    chat_log.push_exploration(name, xp);
    if let Some(kit) = kit {
        sound_out.write(crate::net::ServerSoundMessage {
            kind: crate::net::ServerSoundKind::Sound2d,
            sound_id: kit,
            source: None,
        });
    }
}

/// `PLAYER_FLAGS` bit 25, no XP gain.
const PLAYER_FLAGS_NO_XP_GAIN: u32 = 0x0200_0000;

/// The XP the exploration line shows, 0 for none: a signed positive award to a resolved player
/// without no-XP-gain (`0x6e3733`-`0x6e374e`).
fn exploration_line_xp(xp: u32, player_flags: Option<u32>) -> u32 {
    match player_flags {
        Some(flags) if (xp as i32) > 0 && flags & PLAYER_FLAGS_NO_XP_GAIN == 0 => xp,
        _ => 0,
    }
}

/// `SMSG_LEVELUP_INFO`: the level-gated tutorials, then `PLAYER_LEVEL_UP`, the event's one fire
/// site (`0x6e3691`), parked for `ui_unit` to fire whatever the level field does. The ding's visual
/// rides the `UNIT_FIELD_LEVEL` change watcher, not this packet.
pub(super) fn level_up(
    l: LevelUpInfo,
    chat_log: &mut ChatLog,
    tutorials: Option<&mut MessageWriter<crate::tutorial::TutorialEvent>>,
) {
    if let Some(t) = tutorials {
        for id in crate::tutorial::level_triggers(l.level) {
            t.write(crate::tutorial::TutorialEvent::trigger(id));
        }
    }
    // Stock `ChatFrame_OnEvent` prints the block from the event; nothing here prints it, or it
    // shows twice.
    chat_log.push_level_up(l);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn xp(victim: u64, xp_type: u8, refer_a_friend: bool) -> XpGain {
        XpGain {
            victim,
            total: 52,
            xp_type,
            base: 35,
            group_rate: 1.0,
            refer_a_friend,
        }
    }

    /// Our player at rest state `state` (`PLAYER_BYTES_2` byte 3, field 194).
    fn me(state: u8) -> ObjectStore {
        ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[(
            194,
            u32::from(state) << 24,
        )]))
    }

    fn states() -> RestStates {
        RestStates(vec![
            (1, "Rested".into(), 2.0),
            (2, "Normal".into(), 1.0),
            (6, "Refer-A-Friend".into(), 3.0),
        ])
    }

    /// `0x50c7b0`, `0x50b300`, `0x50b56d`: the loss, rested, plain and RAF quest forms.
    #[test]
    fn the_xp_line_takes_the_reference_route() {
        const ME: u64 = 1;
        const MOB: u64 = 7;
        let held = |g| g == MOB;
        let (rested, normal, rs) = (me(1), me(2), states());
        let route =
            |x: &XpGain, store: &ObjectStore| xp_line(x, Some(ME), Some(store), held, Some(&rs));

        assert_eq!(
            route(&xp(ME, 0, false), &rested),
            XpLine::Loss { total: 52 }
        );
        assert!(matches!(
            route(&xp(MOB, 0, false), &rested),
            XpLine::Rested { key: "COMBATLOG_XPGAIN_EXHAUSTION1", ref state, .. } if state == "Rested"
        ));
        // Factor 1 is no bonus state: the plain line, named.
        assert_eq!(
            route(&xp(MOB, 0, false), &normal),
            XpLine::Plain {
                victim: MOB,
                total: 52,
                group_rate: 1.0
            }
        );
        // RAF forces state 6 whatever the byte says.
        assert!(matches!(
            route(&xp(MOB, 0, true), &normal),
            XpLine::Rested { ref state, .. } if state == "Refer-A-Friend"
        ));
        // A victim we do not hold is the unnamed plain line.
        assert_eq!(
            route(&xp(99, 0, false), &rested),
            XpLine::Plain {
                victim: 0,
                total: 52,
                group_rate: 1.0
            }
        );
        assert_eq!(
            route(&xp(0, 1, true), &rested),
            XpLine::ReferAFriendQuest {
                total: 52,
                state: "Refer-A-Friend".into()
            }
        );
        assert_eq!(
            route(&xp(0, 2, true), &rested),
            XpLine::Plain {
                victim: 0,
                total: 52,
                group_rate: 1.0
            },
            "RAF quest XP is types 1 and 6 only"
        );
    }

    /// `0x6e3733`-`0x6e374e`: the XP line needs a signed positive award, a resolved player and no
    /// no-XP-gain flag.
    #[test]
    fn the_exploration_xp_line_honours_no_xp_gain() {
        assert_eq!(exploration_line_xp(250, Some(0)), 250);
        assert_eq!(exploration_line_xp(250, Some(PLAYER_FLAGS_NO_XP_GAIN)), 0);
        assert_eq!(exploration_line_xp(0, Some(0)), 0);
        assert_eq!(
            exploration_line_xp(u32::MAX, Some(0)),
            0,
            "signed: -1 is no award"
        );
        assert_eq!(exploration_line_xp(250, None), 0);
    }
}
