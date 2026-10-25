//! 12340's `WowTime` (`0x76c190`-`0x76cf10`): a broken-down date whose every field may be unset
//! (-1), packed into one wire word. The reference steps days through the C runtime's `mktime`
//! and `localtime`; this steps them on the proleptic Gregorian calendar, which is the same away
//! from a daylight-saving edge, and `AddHolidayDuration`'s own correction cancels that edge.

use crate::civil;

/// A `WowTime`. Years count from 2000, months and month days from 0, weekdays from Sunday.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WowTime {
    pub minute: i32,
    pub hour: i32,
    pub weekday: i32,
    pub month_day: i32,
    pub month: i32,
    pub year: i32,
    pub flags: i32,
    /// `m_tzHours`, the server's zone offset the clock packet carries; never packed.
    pub tz_hours: i32,
}

/// `WowTime::WowTime` (`0x76c190`): every field unset.
impl Default for WowTime {
    fn default() -> Self {
        Self {
            minute: -1,
            hour: -1,
            weekday: -1,
            month_day: -1,
            month: -1,
            year: -1,
            flags: 0,
            tz_hours: 0,
        }
    }
}

/// One packed field: an all-ones value is unset (`0x76c970`).
fn field(value: u32, shift: u32, bits: u32) -> i32 {
    let mask = (1u32 << bits) - 1;
    match (value >> shift) & mask {
        v if v == mask => -1,
        v => v as i32,
    }
}

impl WowTime {
    /// `WowDecodeTime` (`0x76cab0`).
    pub fn decode(value: u32) -> Self {
        Self {
            minute: field(value, 0, 6),
            hour: field(value, 6, 5),
            weekday: field(value, 11, 3),
            month_day: field(value, 14, 6),
            month: field(value, 20, 4),
            year: field(value, 24, 5),
            flags: field(value, 29, 2),
            tz_hours: 0,
        }
    }

    /// `WowEncodeTime` (`0x76ca50`): an unset field packs as all ones.
    pub fn encode(&self) -> u32 {
        (self.minute as u32 & 0x3f)
            | (self.hour as u32 & 0x1f) << 6
            | (self.weekday as u32 & 0x7) << 11
            | (self.month_day as u32 & 0x3f) << 14
            | (self.month as u32 & 0xf) << 20
            | (self.year as u32 & 0x1f) << 24
            | (self.flags as u32 & 0x3) << 29
    }

    /// A date alone, the clock fields unset, as `EncodeDate` (`0x5b7af0`) builds a table key.
    pub fn date(month: i32, month_day: i32, year: i32) -> Self {
        Self {
            month,
            month_day,
            year,
            ..Self::default()
        }
    }

    /// The day count `mktime` would give the date, normalising an out-of-range month or day.
    fn day_index(&self) -> i64 {
        let month = i64::from(self.month);
        let year = 2000 + i64::from(self.year) + month.div_euclid(12);
        let first = civil::days_from_civil(year, month.rem_euclid(12) as u32 + 1, 1);
        first + i64::from(self.month_day)
    }

    /// Seat the date fields, weekday included, from a day count.
    fn set_day_index(&mut self, days: i64) {
        let c = civil::from_unix(days * 86_400);
        self.year = (c.year - 2000) as i32;
        self.month = c.month as i32 - 1;
        self.month_day = c.day as i32 - 1;
        self.weekday = c.weekday as i32;
    }

    /// `AddDays` (`0x76c280`): nothing unless the date is whole; `keep_time` carries the clock
    /// through the step, else only the date moves.
    pub fn add_days(&mut self, days: i32, keep_time: bool) {
        if self.year < 0 || self.month < 0 || self.month_day < 0 {
            return;
        }
        let day = self.day_index() + i64::from(days);
        if keep_time {
            let total = day * 1440 + i64::from(self.hour) * 60 + i64::from(self.minute);
            self.set_day_index(total.div_euclid(1440));
            let minute_of_day = total.rem_euclid(1440) as i32;
            self.hour = minute_of_day / 60;
            self.minute = minute_of_day % 60;
        } else {
            self.set_day_index(day);
        }
    }

    /// `GetHourAndMinutes` (`0x76c360`): minutes into the day, 0 with either clock field unset.
    pub fn hour_and_minutes(&self) -> i32 {
        if self.hour < 0 || self.minute < 0 {
            return 0;
        }
        self.minute + 60 * self.hour
    }

    /// `SetHourAndMinutes(int)` (`0x76c380`).
    pub fn set_minutes_of_day(&mut self, minutes: i32) {
        self.minute = minutes % 60;
        self.hour = minutes / 60;
    }

    /// `SetHourAndMinutes(hour, minute)` (`0x76c3c0`): refuses an hour past 23 or a minute past 59.
    pub fn set_hour_and_minutes(&mut self, hour: u32, minute: u32) -> bool {
        if hour >= 24 || minute >= 60 {
            return false;
        }
        self.hour = hour as i32;
        self.minute = minute as i32;
        true
    }

    /// `AddMinutes` (`0x76c3f0`).
    pub fn add_minutes(&mut self, minutes: i32) {
        let mut total = self.hour_and_minutes() + minutes;
        let days = total / 1440;
        if days != 0 {
            total -= 1440 * days;
            self.add_days(days, false);
        }
        if total < 0 {
            self.add_days(-1, false);
            total += 1440;
        }
        self.set_minutes_of_day(total);
    }

    /// `SetDate` (`0x76c480`): a year past 2000 is taken as a full year; refuses a month past 11,
    /// a month day past 31 or a year past 2031.
    pub fn set_date(&mut self, month: u32, month_day: u32, year: u32) -> bool {
        if month >= 12 || month_day >= 32 {
            return false;
        }
        let year = if year >= 2000 { year - 2000 } else { year };
        if year > 31 {
            return false;
        }
        self.month = month as i32;
        self.month_day = month_day as i32;
        self.year = year as i32;
        true
    }

    /// `AddHolidayDuration` (`0x76c4c0`): whole days keep the clock, the rest adds minutes, and
    /// an hour gained or lost to the step is put back.
    pub fn add_holiday_duration(&mut self, minutes: i32) {
        let start = *self;
        if minutes / 1440 > 0 {
            self.add_days(minutes / 1440, true);
        }
        self.add_minutes(minutes % 1440);
        let expected = (start.minute + minutes % 1440 + 60 * start.hour) % 1440;
        if expected == self.hour_and_minutes() {
            return;
        }
        if (self.hour_and_minutes() - expected + 1440) % 1440 == 60 {
            if self.hour_and_minutes() < 60 {
                self.add_days(-1, false);
            }
            self.set_minutes_of_day(expected);
        } else {
            self.add_minutes(60);
        }
    }

    /// The fields from the year down, the order every comparison walks.
    fn fields(&self) -> [i32; 6] {
        [
            self.year,
            self.month,
            self.month_day,
            self.weekday,
            self.hour,
            self.minute,
        ]
    }

    /// `operator<` (`0x76c6d0`): the first field both sides set that differs decides.
    pub fn lt(&self, other: &Self) -> bool {
        self.first_difference(other).is_some_and(|(a, b)| a < b)
    }

    /// `operator>` (`0x76c7b0`).
    pub fn gt(&self, other: &Self) -> bool {
        self.first_difference(other).is_some_and(|(a, b)| a > b)
    }

    /// `operator==` (`0x76c890`): every field both sides set agrees; the zone is not compared.
    pub fn same(&self, other: &Self) -> bool {
        self.first_difference(other).is_none()
    }

    /// `operator<=` (`0x76ccc0`).
    pub fn le(&self, other: &Self) -> bool {
        self.same(other) || self.lt(other)
    }

    /// `operator>=` (`0x76cd00`).
    pub fn ge(&self, other: &Self) -> bool {
        self.same(other) || self.gt(other)
    }

    fn first_difference(&self, other: &Self) -> Option<(i32, i32)> {
        self.fields()
            .into_iter()
            .zip(other.fields())
            .find(|&(a, b)| a >= 0 && b >= 0 && a != b)
    }

    /// `ConvertToLocal` (`0x76cba0`): a time stamped in another zone moves by its offset, and its
    /// weekday goes unset. Deviation: the reference adds the host's own offset from UTC too; this
    /// crate has no zone source, so the host reads as UTC (`crate::civil`).
    pub fn convert_to_local(&mut self) {
        if self.tz_hours == 0 {
            return;
        }
        let total = self.day_index() * 1440
            + i64::from(self.hour) * 60
            + i64::from(self.minute)
            + i64::from(self.tz_hours) * 60;
        self.set_day_index(total.div_euclid(1440));
        let minute_of_day = total.rem_euclid(1440) as i32;
        self.hour = minute_of_day / 60;
        self.minute = minute_of_day % 60;
        self.weekday = -1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(year: i32, month: i32, month_day: i32, hour: i32, minute: i32) -> WowTime {
        WowTime {
            year,
            month,
            month_day,
            hour,
            minute,
            ..WowTime::default()
        }
    }

    /// Every field packs where `0x76ca50` puts it and an unset one round-trips as all ones.
    #[test]
    fn a_packed_time_round_trips() {
        let t = WowTime {
            weekday: 6,
            ..at(26, 9, 24, 14, 35)
        };
        assert_eq!(WowTime::decode(t.encode()), t);
        let date = WowTime::date(9, 24, 26);
        assert_eq!(WowTime::decode(date.encode()), date);
        assert_eq!(WowTime::default().encode() & 0x1fff_ffff, 0x1fff_ffff);
    }

    /// Days step across month and year ends and keep the weekday right; the 25th of October 2026
    /// is a Sunday.
    #[test]
    fn days_step_on_the_calendar() {
        let mut t = WowTime::date(9, 24, 26);
        t.add_days(0, false);
        assert_eq!(t.weekday, 0);
        t.add_days(7, false);
        assert_eq!((t.month, t.month_day, t.weekday), (10, 0, 0));
        let mut eve = WowTime::date(11, 30, 26);
        eve.add_days(1, false);
        assert_eq!((eve.year, eve.month, eve.month_day), (27, 0, 0));
        // An out-of-range month normalises as `mktime` does: month 12 is next January.
        let mut wrap = WowTime::date(12, 0, 26);
        wrap.add_days(-1, false);
        assert_eq!((wrap.year, wrap.month, wrap.month_day), (26, 11, 30));
    }

    /// Minutes carry into the date both ways.
    #[test]
    fn minutes_carry_into_the_date() {
        let mut t = at(26, 9, 24, 23, 50);
        t.add_minutes(20);
        assert_eq!((t.month_day, t.hour, t.minute), (25, 0, 10));
        t.add_minutes(-30);
        assert_eq!((t.month_day, t.hour, t.minute), (24, 23, 40));
        t.add_minutes(-3 * 1440);
        assert_eq!((t.month_day, t.hour, t.minute), (21, 23, 40));
    }

    /// The comparisons skip a field either side leaves unset, as a weekly date's year does.
    #[test]
    fn comparisons_skip_unset_fields() {
        let a = at(26, 9, 24, 12, 0);
        let b = at(26, 9, 24, 13, 0);
        assert!(a.lt(&b) && b.gt(&a) && a.le(&b) && !a.same(&b));
        let any_year = WowTime { year: -1, ..b };
        assert!(a.lt(&any_year));
        let clockless = WowTime::date(9, 24, 26);
        assert!(clockless.same(&a) && clockless.le(&a) && clockless.ge(&a));
    }

    /// A holiday duration of whole days keeps the clock.
    #[test]
    fn a_holiday_duration_keeps_the_clock() {
        let mut t = at(26, 9, 30, 6, 0);
        t.add_holiday_duration(3 * 1440 + 90);
        assert_eq!((t.month, t.month_day, t.hour, t.minute), (10, 2, 7, 30));
    }

    /// A zone offset moves the clock and unsets the weekday; zone 0 is untouched.
    #[test]
    fn a_zone_offset_moves_to_local() {
        let mut t = WowTime {
            tz_hours: 3,
            weekday: 2,
            ..at(26, 9, 24, 22, 0)
        };
        t.convert_to_local();
        assert_eq!((t.month_day, t.hour, t.weekday), (25, 1, -1));
        let mut utc = at(26, 9, 24, 22, 0);
        utc.convert_to_local();
        assert_eq!(utc, at(26, 9, 24, 22, 0));
    }
}
