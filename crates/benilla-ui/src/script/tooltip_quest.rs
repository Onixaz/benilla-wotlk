//! The quest tooltip (`0x622960`): `SetHyperlink` on a `quest:` link, for any quest the template
//! cache can answer, in the log or not, read at the call. A template miss leaves the tooltip cleared, and a name in flight reads as the placeholder; each record the build missed (the template, a name in the leaderboard) builds it again when it lands, through the tooltip's quest callback (`0x61de20`).

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

/// A cache record a quest tooltip's build looked up and missed, whose arrival builds it again.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum QuestRecord {
    Quest(u32),
    Creature(u32),
    GameObject(u32),
    Item(u32),
}

/// A quest tooltip's build as the caches answer at the call.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct QuestTooltipRead {
    /// `None` while the template is not cached.
    pub view: Option<QuestTooltipView>,
    /// The records looked up and still in flight.
    pub waits: Vec<QuestRecord>,
}

impl super::UiScript {
    /// Tooltip `h`'s callback on a found item record (`0x61de20`): it builds again for the quest it shows now, if any (`0x61de2d`).
    pub(crate) fn quest_tooltip_callback(&mut self, h: FrameHandle) {
        let quest_id = {
            let mut model = self.model_mut();
            model.quest_tooltip_waits.remove(&h);
            tip_mut(&mut model, h).map_or(0, |t| t.quest_id)
        };
        if quest_id == 0 {
            return;
        }
        if let Err(e) = rebuild(&self.lua, h, quest_id) {
            self.push_error(e);
        }
    }

    /// A cache record landed (`0x61de20`, found): every tooltip whose build missed it builds again for the quest it shows.
    pub fn quest_record_landed(&mut self, record: QuestRecord) {
        let waiting: Vec<FrameHandle> = {
            let model = self.model_ref();
            if model.quest_tooltip_waits.is_empty() {
                return;
            }
            model
                .quest_tooltip_waits
                .iter()
                .filter(|(_, w)| w.contains(&record))
                .map(|(h, _)| *h)
                .collect()
        };
        for h in waiting {
            let quest_id = {
                let mut model = self.model_mut();
                model.quest_tooltip_waits.remove(&h);
                tip_mut(&mut model, h).map_or(0, |t| t.quest_id)
            };
            // The callback builds only a tooltip still showing a quest (`0x61de2d`).
            if quest_id == 0 {
                continue;
            }
            if let Err(e) = rebuild(&self.lua, h, quest_id) {
                self.push_error(e);
            }
        }
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
    // The caches at the call, each miss asked with the tooltip's callback (`0x622984`, `0x622a62`).
    let get = |key: &str| crate::strings::global(lua, key);
    let read = super::write_live(|l| l.quest_tooltip(quest_id, &get)).flatten();
    let (view, active) = {
        let mut model = crate::script::model_mut_of(lua);
        clear_content(&mut model, h);
        tip_mut(&mut model, h)?.quest_id = quest_id;
        // Without a view the template reads as in flight.
        let QuestTooltipRead { view, waits } = read.unwrap_or_else(|| QuestTooltipRead {
            view: None,
            waits: vec![QuestRecord::Quest(quest_id)],
        });
        // An item record's ask leaves the tooltip's callback on the item cache's list, one per build (`0x622984`).
        for w in &waits {
            if let QuestRecord::Item(item) = *w {
                super::item_stats::wait_item_record(
                    &mut model,
                    item,
                    super::ItemRecordWait::QuestTooltip(h),
                );
            }
        }
        if !waits.is_empty() {
            model.quest_tooltip_waits.insert(h, waits);
        }
        let active = super::quest_log::quest_row_of(&model, quest_id).is_some();
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
    use std::collections::HashMap;

    use super::{QuestRecord, QuestTooltipRead, QuestTooltipView};
    use crate::script::{LiveLoan, LiveState, UiScript};

    /// The caches as a test sets them between calls.
    #[derive(Default)]
    struct Caches(HashMap<u32, QuestTooltipRead>);

    impl LiveState for Caches {
        fn unit_flags(&self, _guid: u64) -> Option<u32> {
            None
        }

        fn unit_health(&self, _guid: u64) -> Option<u32> {
            None
        }

        fn active_player(&self) -> Option<u64> {
            None
        }

        fn has_control(&self) -> bool {
            true
        }

        fn far_sight(&self) -> bool {
            false
        }

        fn quest_tooltip(
            &mut self,
            quest_id: u32,
            _get: &dyn Fn(&str) -> Option<String>,
        ) -> Option<QuestTooltipRead> {
            Some(self.0.get(&quest_id).cloned().unwrap_or(QuestTooltipRead {
                view: None,
                waits: vec![QuestRecord::Quest(quest_id)],
            }))
        }
    }

    fn tip() -> (UiScript, LiveLoan<Caches>) {
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
        // SAFETY: each test drops its loan on its own thread.
        let loan = unsafe { LiveLoan::new(Caches::default()) };
        (s, loan)
    }

    fn hogger(requirement: &str) -> QuestTooltipView {
        QuestTooltipView {
            title: "Wanted: Hogger".into(),
            objectives_text: "Bring the paw.".into(),
            requirements: vec![requirement.into()],
        }
    }

    fn cache(loan: &mut LiveLoan<Caches>, read: QuestTooltipRead) {
        loan.with_view_mut(|v| v.0.insert(176, read));
    }

    /// A template miss leaves the tooltip cleared; its answer builds it (`0x61de20`), and a second click hides it.
    #[test]
    fn a_quest_link_waits_on_its_template_then_builds_and_a_second_click_hides() {
        let (mut s, mut caches) = tip();
        s.run(r#"T:SetHyperlink("quest:176:13")"#).unwrap();
        assert_eq!(s.eval::<i64>("return T:NumLines()").unwrap(), 0);
        assert_eq!(s.eval::<i64>("return fired").unwrap(), 0);
        cache(
            &mut caches,
            QuestTooltipRead {
                view: Some(hogger("Huge Gnoll Claw x 1")),
                waits: Vec::new(),
            },
        );
        s.quest_record_landed(QuestRecord::Quest(176));
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

    /// A name in flight shows at once as the placeholder, and its answer builds the tooltip again with the name; another record's answer builds nothing.
    #[test]
    fn a_name_in_flight_shows_the_placeholder_and_its_answer_rebuilds() {
        let (mut s, mut caches) = tip();
        cache(
            &mut caches,
            QuestTooltipRead {
                view: Some(hogger("... slain: 0/1")),
                waits: vec![QuestRecord::Creature(448)],
            },
        );
        s.run(r#"T:SetHyperlink("quest:176:13")"#).unwrap();
        assert_eq!(s.eval::<i64>("return fired").unwrap(), 1);
        assert_eq!(
            s.eval::<String>("return TTextLeft6:GetText()").unwrap(),
            " - ... slain: 0/1"
        );
        cache(
            &mut caches,
            QuestTooltipRead {
                view: Some(hogger("Hogger slain: 0/1")),
                waits: Vec::new(),
            },
        );
        s.quest_record_landed(QuestRecord::Creature(99));
        assert_eq!(s.eval::<i64>("return fired").unwrap(), 1, "not its record");
        s.quest_record_landed(QuestRecord::Creature(448));
        assert_eq!(s.eval::<i64>("return fired").unwrap(), 2);
        assert_eq!(
            s.eval::<String>("return TTextLeft6:GetText()").unwrap(),
            " - Hogger slain: 0/1"
        );
        // Answered once: a second answer finds no wait.
        s.quest_record_landed(QuestRecord::Creature(448));
        assert_eq!(s.eval::<i64>("return fired").unwrap(), 2);
    }

    /// The callback builds only a tooltip still showing a quest (`0x61de2d`).
    #[test]
    fn an_answer_after_the_tooltip_moved_on_builds_nothing() {
        let (mut s, mut caches) = tip();
        s.run(r#"T:SetHyperlink("quest:176:13")"#).unwrap();
        s.run(r#"T:SetOwner(Owner, "ANCHOR_PRESERVE") T:SetText("other")"#)
            .unwrap();
        cache(
            &mut caches,
            QuestTooltipRead {
                view: Some(hogger("Huge Gnoll Claw x 1")),
                waits: Vec::new(),
            },
        );
        s.quest_record_landed(QuestRecord::Quest(176));
        assert_eq!(s.eval::<i64>("return fired").unwrap(), 0);
        assert_eq!(
            s.eval::<String>("return TTextLeft1:GetText()").unwrap(),
            "other"
        );
    }
}
