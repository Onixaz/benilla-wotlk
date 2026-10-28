//! The threat lists the app holds, as the threat verbs read them: per unit with a list (a "mob"),
//! its highest-threat guid and one row per guid on the list. The app keeps the lists and runs
//! their update rules; this side answers the reference's two readers, `CalculateThreat`
//! (`0x7374c0`) for one mob and one guid, and the per-guid situation (`0x52a1a0`) across every
//! mob listing it.

use std::collections::HashMap;

use super::model::Model;

/// One guid's row on a mob's list, as the app last computed it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ThreatRow {
    /// 1 not tanking and below the top, 2 above the top's threat while not tanking, 3 tanking
    /// with someone else above, 4 tanking securely; the verbs answer it less one.
    pub status: u8,
    /// The raw share of the top's threat, 0 to 250, and 255 until first computed (`0x7416f0`).
    pub percent: u8,
    pub threat: i32,
    /// The guid stands within the mob's melee range (`0x7374c0`'s scaling test).
    pub in_melee: bool,
}

/// One mob's list.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MobThreat {
    /// The highest-threat guid, 0 for none.
    pub highest: u64,
    /// The rows in the order they joined the list.
    pub rows: Vec<(u64, ThreatRow)>,
}

impl MobThreat {
    fn row(&self, guid: u64) -> Option<&ThreatRow> {
        self.rows.iter().find(|(g, _)| *g == guid).map(|(_, r)| r)
    }
}

/// `CalculateThreat`'s outputs (`0x7374c0`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Calculated {
    pub(crate) status: u8,
    pub(crate) raw_percent: u8,
    pub(crate) percent: f32,
    pub(crate) threat: i32,
}

/// `CalculateThreat(mob, guid)` (`0x7374c0`). With no highest guid or no row, the status is 1 for
/// a row and 0 for none, the rest 0. Otherwise the row's status, its raw percent, and a percent
/// that is 100 for the top and else the raw one scaled down to the share that pulls aggro: by
/// 1/1.1 within melee range of the mob and 1/1.3 beyond it.
pub(crate) fn calculate(model: &Model, mob: u64, guid: u64) -> Calculated {
    let list = model.threat.get(&mob);
    let row = list.and_then(|l| l.row(guid));
    match (list, row) {
        (Some(list), Some(row)) if list.highest != 0 => Calculated {
            status: row.status,
            raw_percent: row.percent,
            percent: if guid == list.highest {
                100.0
            } else if row.in_melee {
                f32::from(row.percent) * 0.909_090_88
            } else {
                f32::from(row.percent) * 0.769_230_78
            },
            threat: row.threat,
        },
        _ => Calculated {
            status: u8::from(row.is_some()),
            raw_percent: 0,
            percent: 0.0,
            threat: 0,
        },
    }
}

/// The guid's worst situation over every mob listing it (`0x52a1a0`, the map walk `0x519d50`):
/// the highest status up to 2, and past it, 3 (tanking with someone above) wins over 4. The walk
/// order does not change the answer.
pub(crate) fn situation(model: &Model, guid: u64) -> u8 {
    let statuses = model
        .threat
        .keys()
        .map(|&mob| calculate(model, mob, guid).status)
        .filter(|&s| s != 0);
    let mut level = 0;
    for status in statuses {
        if level <= 2 {
            level = level.max(status);
        } else if level == 4 && status == 3 {
            level = 3;
        }
    }
    level
}

/// The mob's highest-threat guid, 0 for none.
pub(crate) fn highest(model: &Model, mob: u64) -> u64 {
    model.threat.get(&mob).map_or(0, |l| l.highest)
}

impl super::UiScript {
    /// Push the threat lists of every held unit that has one, keyed by the unit's guid. The app
    /// fires `UNIT_THREAT_LIST_UPDATE` and `UNIT_THREAT_SITUATION_UPDATE` itself.
    pub fn set_threat(&mut self, lists: HashMap<u64, MobThreat>) {
        set_field!(self, threat, lists);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::UiScript;

    const MOB: u64 = 0xF130_0000_0000_00AB;
    const OTHER_MOB: u64 = 0xF130_0000_0000_00AC;
    const TANK: u64 = 0x10;
    const MELEE: u64 = 0x20;
    const CASTER: u64 = 0x30;

    fn row(status: u8, percent: u8, threat: i32, in_melee: bool) -> ThreatRow {
        ThreatRow {
            status,
            percent,
            threat,
            in_melee,
        }
    }

    fn script() -> UiScript {
        let mut s = UiScript::new().unwrap();
        s.set_threat(HashMap::from([(
            MOB,
            MobThreat {
                highest: TANK,
                rows: vec![
                    (TANK, row(4, 255, 1000, true)),
                    (MELEE, row(1, 80, 800, true)),
                    (CASTER, row(2, 120, 1200, false)),
                ],
            },
        )]));
        s
    }

    #[test]
    fn the_top_reads_100_and_the_rest_scale_by_range() {
        let s = script();
        let model = s.model_ref();
        assert_eq!(calculate(&model, MOB, TANK).percent, 100.0);
        assert_eq!(
            calculate(&model, MOB, TANK).raw_percent,
            255,
            "the top keeps 255 raw"
        );
        let melee = calculate(&model, MOB, MELEE);
        assert_eq!(
            (melee.status, melee.raw_percent, melee.threat),
            (1, 80, 800)
        );
        assert!((melee.percent - 72.727_27).abs() < 1e-3, "80 / 1.1");
        let caster = calculate(&model, MOB, CASTER);
        assert!((caster.percent - 92.307_69).abs() < 1e-3, "120 / 1.3");
    }

    #[test]
    fn a_list_without_a_top_reads_one_for_a_row_and_zero_for_none() {
        let mut s = UiScript::new().unwrap();
        s.set_threat(HashMap::from([(
            MOB,
            MobThreat {
                highest: 0,
                rows: vec![(MELEE, row(2, 90, 5, true))],
            },
        )]));
        let model = s.model_ref();
        assert_eq!(
            calculate(&model, MOB, MELEE),
            Calculated {
                status: 1,
                raw_percent: 0,
                percent: 0.0,
                threat: 0
            }
        );
        assert_eq!(calculate(&model, MOB, CASTER).status, 0);
        assert_eq!(
            calculate(&model, OTHER_MOB, MELEE).status,
            0,
            "no list at all"
        );
    }

    /// `0x519d50`: up to 2 the highest wins, past it 3 wins over 4.
    #[test]
    fn the_situation_prefers_insecure_tanking_over_secure() {
        let mut s = UiScript::new().unwrap();
        let list = |status| MobThreat {
            highest: TANK,
            rows: vec![(TANK, row(status, 255, 10, true))],
        };
        for (statuses, want) in [
            (vec![1, 2], 2),
            (vec![4, 1], 4),
            (vec![4, 3], 3),
            (vec![3, 4], 3),
            (vec![2, 4], 4),
            (vec![], 0),
        ] {
            let lists = statuses
                .iter()
                .enumerate()
                .map(|(i, &st)| (MOB + i as u64, list(st)))
                .collect();
            s.set_threat(lists);
            assert_eq!(situation(&s.model_ref(), TANK), want, "{statuses:?}");
        }
    }
}
