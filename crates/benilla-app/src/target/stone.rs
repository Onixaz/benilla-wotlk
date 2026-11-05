//! A meeting stone's use (`GAMEOBJECT_TYPE_MEETINGSTONE`, 23). 3.3.5a has no meeting-stone queue:
//! the type's table is the animated GameObject's with only its use slot replaced (`0x712270`),
//! which summons the selected party member. Its refusals read the roster, so the click hands the
//! stone here and the shared use, `CMSG_GAMEOBJ_USE`, follows a pass.

use bevy::prelude::*;

use crate::net::{ClientCommand, NetCommands, ObjectStore, SelfPlayer};
use crate::ui_party::GroupState;

/// A right-click on a meeting stone in reach that passed the shared click gates.
#[derive(Message, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MeetingStoneUse {
    pub(crate) go_guid: u64,
}

/// The selected unit as the use reads it: its guid, and its level when its object is in view.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct StoneTarget {
    pub(crate) guid: Option<u64>,
    pub(crate) level: Option<u32>,
}

/// The stone's use slot (`0x712270`): `None` passes to the shared use, else the error it shows.
/// In order: no group, no target, our level outside the stone's `data[0]..=data[1]` (`0x7121e0`),
/// a target outside the group, a target neither in view nor an online member with a level, and the
/// target's level outside the range. An uncached template reads 0/0, which refuses every level.
pub(crate) fn meeting_stone_refusal(
    group: Option<&GroupState>,
    target: StoneTarget,
    level: u32,
    stone: Option<crate::go_templates::MeetingStoneTemplate>,
) -> Option<&'static str> {
    let in_range = |l: u32| stone.is_some_and(|s| s.min_level <= l && l <= s.max_level);
    let group = group.filter(|g| g.in_group && !g.members.is_empty());
    let Some(group) = group else {
        return Some("ERR_MEETING_STONE_NEED_PARTY");
    };
    let Some(guid) = target.guid else {
        return Some("ERR_GENERIC_NO_TARGET");
    };
    if !in_range(level) {
        return Some("ERR_MEETING_STONE_INVALID_LEVEL");
    }
    if !group.members.iter().any(|m| m.guid == guid) {
        return Some("ERR_MEETING_STONE_TARGET_NOT_IN_PARTY");
    }
    let target_level = target.level.or_else(|| {
        let stats = group.stats.get(&guid)?;
        let level = u32::from(stats.level?);
        (level != 0 && stats.is_online()).then_some(level)
    });
    let Some(target_level) = target_level else {
        return Some("ERR_MEETING_STONE_NOT_FOUND");
    };
    (!in_range(target_level)).then_some("ERR_MEETING_STONE_TARGET_INVALID_LEVEL")
}

/// The stone's use, run with the click's guid: a refusal line, or `CMSG_GAMEOBJ_USE`. The shared
/// use's lock arms never apply, as the type has no lock slot.
pub(crate) fn drain_meeting_stone_uses(
    mut uses: MessageReader<MeetingStoneUse>,
    group: Option<Res<GroupState>>,
    selection: Res<super::Selection>,
    templates: Res<crate::go_templates::GameObjectTemplates>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    stores: Query<&ObjectStore>,
    commands: Res<NetCommands>,
    mut errors: ResMut<crate::ui_action::UiErrorKeys>,
) {
    let Some(level) = self_q.single().ok().and_then(|s| s.0.unit_level()) else {
        uses.clear();
        return;
    };
    let target = StoneTarget {
        guid: selection.guid,
        level: selection
            .target
            .and_then(|e| stores.get(e).ok())
            .and_then(|s| s.0.unit_level()),
    };
    for &MeetingStoneUse { go_guid } in uses.read() {
        let stone = templates.get(go_guid).and_then(|t| t.meeting_stone);
        match meeting_stone_refusal(group.as_deref(), target, level, stone) {
            Some(key) => {
                trace_use(&format!("meeting stone {go_guid:#x} refused: {key}"));
                errors.0.push(crate::ui_action::UiError::key(key));
            }
            None => {
                trace_use(&format!("SEND CMSG_GAMEOBJ_USE guid={go_guid:#x}"));
                let _ = commands.0.send(ClientCommand::GameObjUse { guid: go_guid });
            }
        }
    }
}

/// A line on the click's own `use` trace tag.
fn trace_use(line: &str) {
    if benilla_assets::trace::enabled_for("use") {
        benilla_assets::trace::line("use", line);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::go_templates::MeetingStoneTemplate;
    use benilla_protocol::messages::{GroupMemberEntry, PartyMemberStatsInfo};

    const MATE: u64 = 0xa11e;
    const STONE: Option<MeetingStoneTemplate> = Some(MeetingStoneTemplate {
        min_level: 15,
        max_level: 60,
        area: 1519,
    });

    fn party(members: &[u64]) -> GroupState {
        GroupState {
            in_group: true,
            members: members
                .iter()
                .map(|&guid| GroupMemberEntry {
                    name: format!("{guid:x}"),
                    guid,
                    status: 1,
                    flags: 0,
                    roles: 0,
                })
                .collect(),
            ..GroupState::default()
        }
    }

    fn seen(guid: u64, level: u32) -> StoneTarget {
        StoneTarget {
            guid: Some(guid),
            level: Some(level),
        }
    }

    #[test]
    fn the_stone_refuses_in_the_references_order() {
        let g = party(&[MATE]);
        let refusal = |g: Option<&GroupState>, t, level| meeting_stone_refusal(g, t, level, STONE);
        assert_eq!(
            refusal(None, seen(MATE, 40), 40),
            Some("ERR_MEETING_STONE_NEED_PARTY")
        );
        assert_eq!(
            refusal(Some(&GroupState::default()), seen(MATE, 40), 40),
            Some("ERR_MEETING_STONE_NEED_PARTY")
        );
        assert_eq!(
            refusal(Some(&g), StoneTarget::default(), 5),
            Some("ERR_GENERIC_NO_TARGET"),
            "the target before our level"
        );
        assert_eq!(
            refusal(Some(&g), seen(0xbeef, 40), 61),
            Some("ERR_MEETING_STONE_INVALID_LEVEL"),
            "our level before the target's membership"
        );
        assert_eq!(
            refusal(Some(&g), seen(0xbeef, 40), 40),
            Some("ERR_MEETING_STONE_TARGET_NOT_IN_PARTY")
        );
        assert_eq!(
            refusal(Some(&g), seen(MATE, 14), 40),
            Some("ERR_MEETING_STONE_TARGET_INVALID_LEVEL")
        );
        assert_eq!(refusal(Some(&g), seen(MATE, 15), 60), None);
        assert_eq!(
            meeting_stone_refusal(Some(&g), seen(MATE, 40), 40, None),
            Some("ERR_MEETING_STONE_INVALID_LEVEL"),
            "an uncached template reads 0/0"
        );
    }

    #[test]
    fn a_member_out_of_view_is_summoned_by_its_online_record() {
        let mut g = party(&[MATE]);
        let away = StoneTarget {
            guid: Some(MATE),
            level: None,
        };
        let refusal = |g: &GroupState| meeting_stone_refusal(Some(g), away, 40, STONE);
        assert_eq!(refusal(&g), Some("ERR_MEETING_STONE_NOT_FOUND"));
        let mut stats = PartyMemberStatsInfo {
            level: Some(30),
            ..PartyMemberStatsInfo::default()
        };
        g.stats.insert(MATE, stats.clone());
        assert_eq!(refusal(&g), Some("ERR_MEETING_STONE_NOT_FOUND"), "offline");
        stats.set_online(true);
        g.stats.insert(MATE, stats.clone());
        assert_eq!(refusal(&g), None);
        stats.level = Some(70);
        g.stats.insert(MATE, stats.clone());
        assert_eq!(refusal(&g), Some("ERR_MEETING_STONE_TARGET_INVALID_LEVEL"));
        stats.level = Some(0);
        g.stats.insert(MATE, stats);
        assert_eq!(refusal(&g), Some("ERR_MEETING_STONE_NOT_FOUND"));
    }
}
