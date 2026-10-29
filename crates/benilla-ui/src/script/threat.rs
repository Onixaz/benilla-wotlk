//! Each held unit's threat list as the app keeps it from `SMSG_*THREAT*`, and its readers:
//! `UnitThreatSituation` (`0x613a60`), `UnitDetailedThreatSituation` (`0x613b40`) and the unit
//! tooltip's threat line. The lists are plain data; the app owns their updates.

use std::collections::HashMap;

/// One unit on a list (`0x7416f0` creates it at status 1, percent 255, threat 0).
#[derive(Clone, Debug, PartialEq)]
pub struct ThreatEntry {
    pub guid: u64,
    /// 1 under the highest's threat, 2 at or over it (`0x71c3b0`); the highest's own is 3 with
    /// someone at 2, else 4 (`0x737620`). Lua reads it less one.
    pub status: u8,
    /// Threat as a whole percent of the highest's, capped at 250 (`0x71c3b0`); the highest's own
    /// is never recomputed and keeps what it held, 255 from creation.
    pub percent: u8,
    /// The server's value as sent, read signed.
    pub threat: i32,
    /// Within the list owner's melee range, `max(reachA + reachB + 4/3, 5)` (`0x4f5f40`) against
    /// the squared distance, which scales the percent to pull (`0x737560`); the app measures it.
    pub in_melee: bool,
}

/// One unit's list, its entries in creation order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ThreatList {
    /// The highest-threat unit `SMSG_HIGHEST_THREAT_UPDATE` named, 0 for none (`+0xfd8`).
    pub highest: u64,
    pub entries: Vec<ThreatEntry>,
}

/// What `0x7374c0` answers for one unit on a list.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ThreatReading {
    /// The entry's status, 0 off the list.
    pub status: u8,
    pub raw_percent: u8,
    /// The percent to pull: 100 for the highest, else the raw percent over 1.1 in melee range
    /// or 1.3 outside it, in single precision.
    pub percent: f32,
    pub threat: i32,
}

impl ThreatList {
    pub fn entry(&self, guid: u64) -> Option<&ThreatEntry> {
        self.entries.iter().find(|e| e.guid == guid)
    }

    /// `0x7374c0`: with no highest or no entry, a status of 1 for an entry and 0 for none, the
    /// rest zero, and `false`; else the entry's reading and `true`.
    pub fn calculate(&self, guid: u64) -> (bool, ThreatReading) {
        let entry = self.entry(guid);
        let Some(e) = entry.filter(|_| self.highest != 0) else {
            let status = u8::from(entry.is_some());
            return (
                false,
                ThreatReading {
                    status,
                    ..Default::default()
                },
            );
        };
        let percent = if guid == self.highest {
            100.0
        } else if e.in_melee {
            f32::from(e.percent) * 0.909_090_9
        } else {
            f32::from(e.percent) * 0.769_230_8
        };
        (
            true,
            ThreatReading {
                status: e.status,
                raw_percent: e.percent,
                percent,
                threat: e.threat,
            },
        )
    }
}

/// Every held unit's list, by its guid; a unit that despawns takes its list with it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ThreatView {
    pub lists: HashMap<u64, ThreatList>,
}

impl ThreatView {
    /// `0x52a1a0` over `0x519d50`: the highest status `guid` holds on any list, where a 3 (tanking
    /// with someone at or over it) outranks a 4 (tanking alone); 0 on none.
    pub fn max_level(&self, guid: u64) -> u8 {
        let mut level = 0;
        for list in self.lists.values() {
            if list.entry(guid).is_none() {
                continue;
            }
            let status = list.calculate(guid).1.status;
            if level <= 2 {
                level = level.max(status);
            } else if level == 4 && status == 3 {
                level = 3;
            }
        }
        level
    }
}

impl super::UiScript {
    /// Replace the threat lists; the app fires the threat events itself.
    pub fn set_threat(&mut self, view: ThreatView) {
        set_field!(self, threat, view);
    }

    /// `IsThreatWarningEnabled`'s flag (`0x519df0`), which gates the tooltip's threat line, the
    /// threat tutorial and the overhead threat text.
    pub fn threat_warning_enabled(&self) -> bool {
        super::instance::threat_warning_enabled(&self.model_ref())
    }
}

/// The unit tooltip's threat line for `mob`: with the warning on and the player on its list with a
/// highest, `THREAT_TOOLTIP` filled with `trunc(percent - 0.5)`, and the colour of the status, 1
/// for one past the table.
pub(super) fn tooltip_line(model: &super::Model, mob: u64) -> Option<(i64, u8)> {
    let me = model.unit_guids.player;
    if me == 0 || !super::instance::threat_warning_enabled(model) {
        return None;
    }
    let (ok, r) = model.threat.lists.get(&mob)?.calculate(me);
    ok.then(|| {
        let status = if r.status >= 5 { 1 } else { r.status };
        ((r.percent - 0.5) as i64, status)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ME: u64 = 0x10;
    const PAL: u64 = 0x2A;
    const MOB: u64 = 0xF130_0000_0C35_0001;
    const BOAR: u64 = 0xF130_0000_0C35_0002;

    fn entry(guid: u64, status: u8, percent: u8, threat: i32, in_melee: bool) -> ThreatEntry {
        ThreatEntry {
            guid,
            status,
            percent,
            threat,
            in_melee,
        }
    }

    /// The highest reads 100, a melee entry its raw over 1.1, a ranged one over 1.3.
    #[test]
    fn calculate_scales_the_percent_to_pull_by_range() {
        let list = ThreatList {
            highest: ME,
            entries: vec![
                entry(ME, 4, 255, 15_000, true),
                entry(PAL, 1, 66, 9_900, false),
                entry(BOAR, 1, 66, 9_900, true),
            ],
        };
        let (ok, me) = list.calculate(ME);
        assert!(ok);
        assert_eq!((me.status, me.raw_percent, me.percent), (4, 255, 100.0));
        let pal = list.calculate(PAL).1;
        assert_eq!(pal.percent, 66.0 * 0.769_230_8f32);
        assert_eq!(list.calculate(BOAR).1.percent, 66.0 * 0.909_090_9f32);
    }

    /// No highest: an entry reads status 1 and the rest zero; no entry reads status 0.
    #[test]
    fn calculate_without_a_highest_reads_presence_only() {
        let list = ThreatList {
            highest: 0,
            entries: vec![entry(PAL, 2, 120, 500, true)],
        };
        assert_eq!(
            list.calculate(PAL),
            (
                false,
                ThreatReading {
                    status: 1,
                    ..Default::default()
                }
            )
        );
        assert_eq!(list.calculate(ME).1.status, 0);
    }

    /// Tanking with someone at or over (3) outranks tanking alone (4), which outranks the rest.
    #[test]
    fn max_level_prefers_an_insecure_tank() {
        let mut view = ThreatView::default();
        view.lists.insert(
            MOB,
            ThreatList {
                highest: ME,
                entries: vec![entry(ME, 4, 255, 100, true)],
            },
        );
        view.lists.insert(
            BOAR,
            ThreatList {
                highest: PAL,
                entries: vec![entry(PAL, 4, 255, 100, true), entry(ME, 2, 104, 104, true)],
            },
        );
        assert_eq!(view.max_level(ME), 4);
        view.lists.get_mut(&MOB).unwrap().entries[0].status = 3;
        assert_eq!(view.max_level(ME), 3);
        assert_eq!(view.max_level(0x99), 0);
    }
}
