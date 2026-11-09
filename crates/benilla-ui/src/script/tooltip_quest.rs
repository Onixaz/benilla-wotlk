//! The quest tooltip (`0x622960`): `SetHyperlink` on a `quest:` link, for any quest the template
//! cache can answer, in the log or not. The app reads the cache; a miss leaves the tooltip cleared
//! until the record lands, when the tooltip's quest callback (`0x61de20`) builds it again.

use mlua::{Lua, Table};

use super::object::{frame_handle_of, frame_wrapper};
use super::tooltip::{append_line, clear_content, fire_cleared, set_shown, tip_mut};
use crate::widget::FrameHandle;

const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
const GOLD: [f32; 4] = [1.0, 210.0 / 255.0, 0.0, 1.0];
const GREEN: [f32; 4] = [0.0, 1.0, 0.0, 1.0];

/// One quest as its tooltip reads it, app-resolved.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct QuestTooltipView {
    pub title: String,
    /// The objectives paragraph, its `$` tokens expanded.
    pub objectives_text: String,
    /// The leaderboard's lines in their no-progress form.
    pub requirements: Vec<String>,
}

impl super::UiScript {
    /// The quests a tooltip is waiting on, oldest answer first; the asks stay until answered.
    pub fn quest_tooltip_asks(&self) -> Vec<u32> {
        self.model_ref()
            .quest_tooltip_asks
            .iter()
            .copied()
            .collect()
    }

    /// A record landed: every tooltip waiting on `quest_id` builds again.
    pub fn set_quest_tooltip(&mut self, quest_id: u32, view: QuestTooltipView) {
        let waiting: Vec<FrameHandle> = {
            let mut model = self.model_mut();
            model.quest_tooltip_asks.remove(&quest_id);
            model.quest_tooltips.insert(quest_id, view);
            model
                .quest_tooltip_waits
                .iter()
                .filter(|(_, &q)| q == quest_id)
                .map(|(h, _)| *h)
                .collect()
        };
        for h in waiting {
            if let Err(e) = rebuild(&self.lua, h, quest_id) {
                self.push_error(e);
            }
        }
    }

    /// The server does not know `quest_id`: the ask closes and the tooltip stays cleared.
    pub fn drop_quest_tooltip_ask(&mut self, quest_id: u32) {
        let mut model = self.model_mut();
        model.quest_tooltip_asks.remove(&quest_id);
        model.quest_tooltip_waits.retain(|_, q| *q != quest_id);
    }
}

fn rebuild(lua: &Lua, h: FrameHandle, quest_id: u32) -> mlua::Result<()> {
    let id = crate::script::model_mut_of(lua).frame_id(h);
    let this = frame_wrapper(lua, id)?;
    set_quest(lua, &this, quest_id)
}

/// The builder: the title, the active line for a quest in the log, the objectives paragraph and
/// the requirements, then `OnTooltipSetQuest` before the show.
pub(super) fn set_quest(lua: &Lua, this: &Table, quest_id: u32) -> mlua::Result<()> {
    let h = frame_handle_of(lua, this)?;
    let (view, active) = {
        let mut model = crate::script::model_mut_of(lua);
        clear_content(&mut model, h);
        tip_mut(&mut model, h)?.quest_id = quest_id;
        let view = model.quest_tooltips.get(&quest_id).cloned();
        if view.is_none() {
            model.quest_tooltip_asks.insert(quest_id);
            model.quest_tooltip_waits.insert(h, quest_id);
        }
        let active = model
            .quest_log
            .entries
            .iter()
            .any(|e| !e.is_header && e.quest_id == quest_id);
        (view, active)
    };
    fire_cleared(lua, h);
    let Some(view) = view else {
        return Ok(());
    };
    append_line(lua, this, (view.title.clone(), WHITE), None, false)?;
    if active {
        if let Some(text) = crate::strings::global(lua, "QUEST_TOOLTIP_ACTIVE") {
            append_line(lua, this, (text, GREEN), None, false)?;
        }
    }
    append_line(lua, this, (" ".into(), WHITE), None, false)?;
    append_line(lua, this, (view.objectives_text.clone(), GOLD), None, true)?;
    if !view.requirements.is_empty() {
        append_line(lua, this, (" ".into(), WHITE), None, false)?;
        if let Some(text) = crate::strings::global(lua, "QUEST_TOOLTIP_REQUIREMENTS") {
            append_line(lua, this, (text, WHITE), None, false)?;
        }
        for line in &view.requirements {
            append_line(lua, this, (format!(" - {line}"), GOLD), None, false)?;
        }
    }
    let id = crate::script::model_mut_of(lua).frame_id(h);
    if let Err(e) = super::event::fire_widget_handler(lua, id, "OnTooltipSetQuest", Vec::new()) {
        crate::script::model_mut_of(lua).record_lua_error(&e);
    }
    set_shown(lua, h, true);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::QuestTooltipView;
    use crate::script::UiScript;

    fn tip() -> UiScript {
        let mut s = UiScript::new().unwrap();
        s.set_screen_size(800.0, 600.0);
        s.run(
            r#"
            QUEST_TOOLTIP_ACTIVE = "ACTIVE"
            QUEST_TOOLTIP_REQUIREMENTS = "REQS"
            fired = 0
            T = CreateFrame("GameTooltip", "T")
            CreateFrame("Frame", "Owner")
            T:SetOwner(Owner, "ANCHOR_PRESERVE")
            T:SetScript("OnTooltipSetQuest", function() fired = fired + 1 end)
            "#,
        )
        .unwrap();
        s
    }

    #[test]
    fn a_quest_link_asks_then_builds_and_a_second_click_hides() {
        let mut s = tip();
        s.run(r#"T:SetHyperlink("quest:176:13")"#).unwrap();
        assert_eq!(s.quest_tooltip_asks(), vec![176]);
        assert_eq!(s.eval::<i64>("return T:NumLines()").unwrap(), 0);
        s.set_quest_tooltip(
            176,
            QuestTooltipView {
                title: "Wanted: Hogger".into(),
                objectives_text: "Bring the paw.".into(),
                requirements: vec!["Huge Gnoll Claw x 1".into()],
            },
        );
        assert!(s.quest_tooltip_asks().is_empty());
        assert_eq!(s.eval::<i64>("return fired").unwrap(), 1);
        assert_eq!(s.eval::<i64>("return T:NumLines()").unwrap(), 6);
        assert_eq!(
            s.eval::<String>("return TTextLeft6:GetText()").unwrap(),
            " - Huge Gnoll Claw x 1"
        );
        assert!(s.eval::<bool>("return T:IsShown() == 1").unwrap());
        s.run(r#"T:SetHyperlink("quest:176:13")"#).unwrap();
        assert!(s.eval::<bool>("return T:IsShown() == nil").unwrap());
    }

    #[test]
    fn an_unknown_quest_closes_its_ask() {
        let mut s = tip();
        s.run(r#"T:SetHyperlink("quest:999999:1")"#).unwrap();
        s.drop_quest_tooltip_ask(999_999);
        assert!(s.quest_tooltip_asks().is_empty());
    }
}
