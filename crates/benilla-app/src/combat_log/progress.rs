//! The progression lines the combat-log module also handles: the XP number and line, the
//! exploration toast and jingle, and the level-up block.

use bevy::prelude::*;

use benilla_protocol::messages::{ExplorationXp, LevelUpInfo, XpGain};

use crate::combat_text::CombatTextSpawn;
use crate::net::{GuidIndex, ObjectStore, SelfGuid};
use crate::ui_chat::ChatLog;

/// `SMSG_LOG_XPGAIN`: `"XP: %d"` over self, category 4, the one emitter that skips Gate A, for kill
/// and quest XP alike; the packet also writes the chat line.
pub(super) fn xp_gain(
    x: XpGain,
    index: &GuidIndex,
    self_guid: &SelfGuid,
    text: &mut MessageWriter<CombatTextSpawn>,
    chat_log: &mut ChatLog,
) {
    if let Some(&me) = self_guid.0.as_ref().and_then(|g| index.0.get(g)) {
        text.write(CombatTextSpawn {
            anchor: me,
            text: format!("XP: {}", x.total),
            category: 4,
            color: None, // row 4's own purple: XP skips the damage colour law
        });
    }
    chat_log.push_xp_gain(&x);
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
