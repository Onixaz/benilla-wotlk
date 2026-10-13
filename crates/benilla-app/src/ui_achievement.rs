//! The player's achievement state as far as the stock micro bar reads it: the list arrived
//! (`SMSG_ALL_ACHIEVEMENT_DATA`, `0x5b32f0`) and the completed achievements, which an
//! `SMSG_ACHIEVEMENT_EARNED` for the player adds to (`0x5b3075`). Criteria, the achievement
//! window and its events are not modelled.

use std::collections::HashSet;

use bevy::prelude::*;

use benilla_protocol::{SessionEvent, SessionEventKind};
use benilla_ui::script::UiScript;

use crate::net::{NetHandlerApp, SelfGuid};
use crate::ui_unit::UnitFeed;

/// The completed achievements, and whether the list has arrived this session.
#[derive(Resource, Default)]
pub(crate) struct PlayerAchievements {
    received: bool,
    completed: HashSet<u32>,
}

pub(crate) struct UiAchievementPlugin;

impl Plugin for UiAchievementPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PlayerAchievements>()
            .net_handler(
                SessionEventKind::AllAchievementData,
                on_all_achievement_data,
            )
            .net_handler(SessionEventKind::AchievementEarned, on_achievement_earned)
            .add_systems(Update, feed_achievements.in_set(UnitFeed));
    }
}

/// The login list replaces the held set (`0x5b330c` clears it first).
fn on_all_achievement_data(In(ev): In<SessionEvent>, mut held: ResMut<PlayerAchievements>) {
    let SessionEvent::AllAchievementData(data) = ev else {
        return;
    };
    debug!(
        "ui_achievement: {} completed achievement(s)",
        data.completed.len()
    );
    held.received = true;
    held.completed = data.completed.into_iter().collect();
}

fn on_achievement_earned(
    In(ev): In<SessionEvent>,
    mut held: ResMut<PlayerAchievements>,
    self_guid: Res<SelfGuid>,
) {
    let SessionEvent::AchievementEarned(earned) = ev else {
        return;
    };
    if self_guid.0 == Some(earned.player) {
        held.completed.insert(earned.achievement);
    }
}

fn feed_achievements(
    script: Option<NonSendMut<UiScript>>,
    held: Res<PlayerAchievements>,
    mut memory: Local<crate::ui_script::VmMemo<()>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let (_, vm_reset) = memory.get_reset(&script);
    if vm_reset || held.is_changed() {
        script.set_achievements(held.received, held.completed.len());
    }
}
