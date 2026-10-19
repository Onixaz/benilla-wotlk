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

/// `SMSG_EXPLORATION_EXPERIENCE`: the ERR_ZONE_EXPLORED toast and, when xp > 0, the
/// ERR_ZONE_EXPLORED_XP chat line, via [`ChatLog::push_exploration`]; an `AreaTable.dbc` id with
/// no name shows no message. The race's discovery jingle (`ChrRaces.dbc` col 3 to SoundEntries)
/// plays even when the area resolves to nothing, through the same 2D path as `SMSG_PLAY_SOUND`.
/// No floating text: the XP arrives separately as `SMSG_LOG_XPGAIN`.
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
    let race = self_guid
        .0
        .as_ref()
        .and_then(|g| index.0.get(g))
        .and_then(|&e| stores.get(e).ok())
        .and_then(|s| s.0.unit_race());
    let kit = race.and_then(|r| sounds.and_then(|s| s.0.kit(u32::from(r))));
    if let Some(kit) = kit {
        sound_out.write(crate::net::ServerSoundMessage {
            kind: crate::net::ServerSoundKind::Sound2d,
            sound_id: kit,
            source: None,
        });
    }
    match area_table.and_then(|t| t.0.name(x.area_id)) {
        Some(name) => {
            info!(
                "exploration: discovered {name} (area {}, {} xp, jingle kit {})",
                x.area_id,
                x.xp,
                kit.unwrap_or(0)
            );
            chat_log.push_exploration(name, x.xp);
        }
        None => warn!(
            "exploration: discovered area {} has no AreaTable name — no message (sound only)",
            x.area_id
        ),
    }
}

/// `SMSG_LEVELUP_INFO`: the ding's chat lines. The talent-point arg is not on the wire; the client
/// computes `(newLevel >= 10) ? 1 : 0`. The ding's visual rides the `UNIT_FIELD_LEVEL` change
/// watcher, not this packet.
pub(super) fn level_up(l: LevelUpInfo, chat_log: &mut ChatLog) {
    let talent_points = u32::from(l.level >= 10);
    // Parked for `ui_unit`'s `PLAYER_LEVEL_UP`, whose args the stock `ChatFrame_OnEvent` prints;
    // nothing here prints the block itself, or it shows twice.
    chat_log.push_level_up_gains(&l, talent_points);
}
