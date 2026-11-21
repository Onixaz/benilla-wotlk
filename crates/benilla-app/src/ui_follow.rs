//! The follow bridge: drains `FollowUnit` into [`crate::player::FollowRequest`]
//! and fires `AUTOFOLLOW_BEGIN`/`AUTOFOLLOW_END` on [`crate::player::FollowState`]'s transitions,
//! the reference's whole follow interface to the UI (there is no `IsFollowing()`).
//!
//! Deviation: every stop fires `END`, because the reference's three silent stops are a bug: two
//! writers reset the follow mode cell directly, leaving a stale guid, and the canceller's early-out
//! fires nothing.

use bevy::prelude::*;

use benilla_ui::script::{FollowRequest as ScriptFollowRequest, ScriptValue, UiScript};

use crate::player::{FollowRequest, FollowState};
use crate::ui_script::{UiDrain, UiFeed};

/// The followee guid each VM was last told of, so the events fire on transitions only; a VM
/// replaced by `/reload` mid-follow starts at `None` and is told `BEGIN` again.
#[derive(Resource, Default)]
pub(crate) struct FollowFeed {
    vm: crate::ui_script::VmMemo<FollowFeedMemo>,
}

#[derive(Default)]
struct FollowFeedMemo {
    told: Option<u64>,
}

/// Exclusive: the events fire with the `World` lent, so a listener's verb reads
/// and writes it in its call; no borrow is held across a raise.
fn feed_follow(world: &mut World) {
    let Some(vm) = world
        .get_non_send_resource::<UiScript>()
        .map(UiScript::share)
    else {
        return;
    };
    let (guid, name) = {
        let follow = world.resource::<FollowState>();
        (follow.guid, follow.name.clone())
    };
    let told = world.resource_mut::<FollowFeed>().vm.get(&vm).told;
    let (fires, now): (Vec<(&'static str, Vec<ScriptValue>)>, Option<u64>) = match (guid, told) {
        // Started, or switched subject: a switch fires END first, as the reference's arm routine
        // calls the canceller before arming.
        (Some(guid), told) if told != Some(guid) => {
            let mut fires = Vec::new();
            if told.is_some() {
                fires.push(("AUTOFOLLOW_END", vec![]));
            }
            fires.push(("AUTOFOLLOW_BEGIN", vec![ScriptValue::Str(name)]));
            (fires, Some(guid))
        }
        // Stopped. END carries no name: `AutoFollowStatus` (`ZoneText.lua`) reuses BEGIN's.
        (None, Some(_)) => (vec![("AUTOFOLLOW_END", vec![])], None),
        _ => return,
    };
    for (event, args) in fires {
        crate::ui_script::safe_world::raise(world, event, args);
    }
    world.resource_mut::<FollowFeed>().vm.get(&vm).told = now;
}

fn drain_follow(script: Option<NonSendMut<UiScript>>, mut out: MessageWriter<FollowRequest>) {
    let Some(mut script) = script else {
        return;
    };
    for ScriptFollowRequest { name, exact } in script.take_follow_requests() {
        out.write(FollowRequest { name, exact });
    }
}

pub(crate) struct UiFollowPlugin;

impl Plugin for UiFollowPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FollowFeed>().add_systems(
            Update,
            (feed_follow.in_set(UiFeed), drain_follow.in_set(UiDrain)),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    /// The converted feed fires `AUTOFOLLOW_BEGIN` with the `World` lent: the listener reads the
    /// followee through the loan in its call, and an END on the stop; nothing fires twice.
    #[test]
    fn the_exclusive_follow_feed_fires_with_the_world_lent() {
        let mut world = World::new();
        world.init_resource::<FollowFeed>();
        let mut follow = FollowState::default();
        follow.guid = Some(42);
        follow.name = "Bob".into();
        world.insert_resource(follow);
        let script = UiScript::new().unwrap();
        let lua = script.lua().clone();
        let read = lua
            .create_function(|_, ()| {
                Ok(
                    crate::ui_script::safe_world::with_world(|w| w.resource::<FollowState>().guid)
                        .flatten(),
                )
            })
            .unwrap();
        lua.globals().set("LentFollowee", read).unwrap();
        script
            .run(
                r#"SEEN = {}
                   local f = CreateFrame("Frame")
                   f:RegisterEvent("AUTOFOLLOW_BEGIN")
                   f:RegisterEvent("AUTOFOLLOW_END")
                   f:SetScript("OnEvent", function(self, event, name)
                       table.insert(SEEN, event .. ":" .. tostring(name) .. ":" .. tostring(LentFollowee()))
                   end)"#,
            )
            .unwrap();
        world.insert_non_send_resource(script);
        world.run_system_once(feed_follow).unwrap();
        world.run_system_once(feed_follow).unwrap();
        world.resource_mut::<FollowState>().guid = None;
        world.run_system_once(feed_follow).unwrap();
        let seen: String = world
            .non_send_resource::<UiScript>()
            .eval("return table.concat(SEEN, '|')")
            .unwrap();
        assert_eq!(seen, "AUTOFOLLOW_BEGIN:Bob:42|AUTOFOLLOW_END:nil:nil");
    }
}
