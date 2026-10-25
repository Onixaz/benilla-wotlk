//! The two calendar globals the stock clock button reads on every world entry
//! (`GameTimeFrame_OnEvent`, `GameTime.lua:155-168`): `CalendarGetDate` (`0x5b8160`), today's game
//! date, and `CalendarGetNumPendingInvites` (`0x5bf830`). The calendar window's own verbs are
//! not here.

use mlua::Lua;

use super::ScriptValue;

impl super::UiScript {
    /// Today's game date, 0-based as the client's clock keeps it (`0xd37fa0`-`0xd37fac`): the
    /// weekday (0 Sunday), the day of the month, the month and the years since 2000.
    pub fn set_calendar_date(&mut self, weekday: u8, month_day: u8, month: u8, year: u8) {
        set_field!(self, calendar_date, (weekday, month_day, month, year));
    }

    /// `SMSG_CALENDAR_SEND_NUM_PENDING` (`CGCalendar::UpdatePendingInvites`, `0x5b7ce0`): a changed
    /// count fires `CALENDAR_UPDATE_PENDING_INVITES`, and the calendar dispatcher then fires
    /// `CALENDAR_ACTION_PENDING` false (`0x5c404a`).
    pub fn set_calendar_pending_invites(&mut self, count: u32) {
        let changed = {
            let mut model = self.model_mut();
            let changed = model.calendar_pending_invites != count;
            model.calendar_pending_invites = count;
            changed
        };
        if changed {
            self.fire_event("CALENDAR_UPDATE_PENDING_INVITES", vec![]);
        }
        self.fire_event("CALENDAR_ACTION_PENDING", vec![ScriptValue::Bool(false)]);
    }
}

/// Register `CalendarGetDate` and `CalendarGetNumPendingInvites`.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();
    // Weekday, month and day 1-based, the year whole: four numbers.
    g.set(
        "CalendarGetDate",
        lua.create_function(|lua, ()| {
            let (weekday, month_day, month, year) = crate::script::model_mut_of(lua).calendar_date;
            Ok((
                f64::from(weekday) + 1.0,
                f64::from(month) + 1.0,
                f64::from(month_day) + 1.0,
                f64::from(year) + 2000.0,
            ))
        })?,
    )?;
    // The larger of the invites held in the event list and the server's count
    // (`CGCalendar::CountPendingInvites`, `0x5bec70`); the list fills only when the calendar asks
    // for it, which no verb here does, so the count is the server's.
    g.set(
        "CalendarGetNumPendingInvites",
        lua.create_function(|lua, ()| {
            Ok(f64::from(
                crate::script::model_mut_of(lua).calendar_pending_invites,
            ))
        })?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::script::UiScript;

    #[test]
    fn the_date_answers_one_based_with_the_whole_year() {
        let mut s = UiScript::new().unwrap();
        assert_eq!(
            s.eval::<(i64, i64, i64, i64)>("return CalendarGetDate()")
                .unwrap(),
            (1, 1, 1, 2000),
            "the cleared clock, before the server's date"
        );
        s.set_calendar_date(6, 24, 9, 26);
        assert_eq!(
            s.eval::<(i64, i64, i64, i64)>("return CalendarGetDate()")
                .unwrap(),
            (7, 10, 25, 2026)
        );
    }

    #[test]
    fn a_changed_count_fires_its_update_and_every_count_ends_the_action() {
        let mut s = UiScript::new().unwrap();
        s.run(
            "SEEN = {} local f = CreateFrame('Frame') \
             f:RegisterEvent('CALENDAR_UPDATE_PENDING_INVITES') \
             f:RegisterEvent('CALENDAR_ACTION_PENDING') \
             f:SetScript('OnEvent', function(self, event) table.insert(SEEN, event) end)",
        )
        .unwrap();
        s.set_calendar_pending_invites(2);
        s.set_calendar_pending_invites(2);
        assert_eq!(
            s.eval::<i64>("return CalendarGetNumPendingInvites()")
                .unwrap(),
            2
        );
        assert_eq!(
            s.eval::<String>("return table.concat(SEEN, ' ')").unwrap(),
            "CALENDAR_UPDATE_PENDING_INVITES CALENDAR_ACTION_PENDING CALENDAR_ACTION_PENDING"
        );
    }
}
