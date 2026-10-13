//! The two achievement verbs the stock micro bar reads (`MainMenuBarMicroButtons.lua`,
//! `UpdateMicroButtons`): whether the achievement list arrived and whether any achievement is
//! complete. The app pushes both from `SMSG_ALL_ACHIEVEMENT_DATA` and `SMSG_ACHIEVEMENT_EARNED`.

use mlua::Lua;

use super::Model;

impl super::UiScript {
    /// Push the achievement state: the list arrived (`[0xc1f454]`, set by `0x5b32fc`) and the
    /// count of completed achievements the client holds (the list at `[0xacfdb8]`).
    pub fn set_achievements(&mut self, received: bool, completed: usize) {
        let mut model = self.model_mut();
        model.achievement_list_received = received;
        model.achievements_completed = completed;
    }
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // CanShowAchievementUI() (`0x5b0ce0`): the list has arrived.
    g.set(
        "CanShowAchievementUI",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.achievement_list_received)
        })?,
    )?;

    // HasCompletedAnyAchievement() (`0x5b1ec0`): the completed list is not empty.
    g.set(
        "HasCompletedAnyAchievement",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.achievements_completed > 0)
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::script::UiScript;

    #[test]
    fn both_verbs_read_the_pushed_state() {
        let mut s = UiScript::new().unwrap();
        assert!(!s
            .eval::<bool>("return CanShowAchievementUI() or HasCompletedAnyAchievement()")
            .unwrap());
        s.set_achievements(true, 0);
        assert!(s.eval::<bool>("return CanShowAchievementUI()").unwrap());
        assert!(!s
            .eval::<bool>("return HasCompletedAnyAchievement()")
            .unwrap());
        s.set_achievements(true, 3);
        assert!(s
            .eval::<bool>("return HasCompletedAnyAchievement()")
            .unwrap());
    }
}
