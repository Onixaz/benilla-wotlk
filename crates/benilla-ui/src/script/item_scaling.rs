//! Scaling items (heirlooms): a template with a nonzero scaling mask (`ItemRec+0xbc`) reads its
//! stats, armor, damage and spell power at a shown level off two tables the app pushes whole,
//! `ScalingStatDistribution` by the template's id (`ItemRec+0xb8`) and `ScalingStatValues` by
//! index, level minus 1 (`0x61f582`, `0x61f5ae`).

use std::collections::HashMap;

use super::ItemTemplateView;

/// One `ScalingStatDistribution.dbc` row.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScalingDistributionView {
    /// `ItemModType` per slot; 0 or below adds nothing to the summary (`0x61f608`).
    pub stat_ids: [i32; 10],
    /// Each slot's share of the budget, in ten-thousandths.
    pub bonuses: [i32; 10],
    /// The highest level the item scales to (`row + 0x54`).
    pub max_level: i32,
}

/// One `ScalingStatValues.dbc` row's 24 columns, column `n` at byte `n * 4` as 12340 reads it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScalingValuesView(pub [i32; 24]);

/// Both tables, pushed once at load.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScalingTables {
    pub distributions: HashMap<u32, ScalingDistributionView>,
    /// In file order: 12340 reads row `level - 1`.
    pub values: Vec<ScalingValuesView>,
}

impl ScalingTables {
    /// The values row a level reads (`0x61f5a7`): `level - 1` in range, else none.
    pub(super) fn values_at(&self, level: i32) -> Option<&ScalingValuesView> {
        usize::try_from(level.checked_sub(1)?)
            .ok()
            .and_then(|i| self.values.get(i))
    }
}

impl ScalingValuesView {
    /// The stat budget by mask `& 0x4001f` (`0x61a650`): one bit picks a column, anything else 0.
    pub(super) fn budget(&self, mask: u32) -> i32 {
        match mask & 0x4_001f {
            0x1 => self.0[2],
            0x2 => self.0[3],
            0x4 => self.0[4],
            0x8 => self.0[17],
            0x10 => self.0[5],
            0x4_0000 => self.0[18],
            _ => 0,
        }
    }

    /// The armor by mask `& 0xf801e0` (`0x61a6b0`).
    pub(super) fn armor(&self, mask: u32) -> i32 {
        match mask & 0xf8_01e0 {
            0x20 => self.0[6],
            0x40 => self.0[7],
            0x80 => self.0[8],
            0x100 => self.0[9],
            0x8_0000 => self.0[19],
            0x10_0000 => self.0[20],
            0x20_0000 => self.0[21],
            0x40_0000 => self.0[22],
            0x80_0000 => self.0[23],
            _ => 0,
        }
    }

    /// The weapon's DPS band by mask `& 0x7e00` (`0x61a830`): the column's DPS less and plus 30%,
    /// 20% for the two-handed columns; no bit is a zero band.
    pub(super) fn damage(&self, mask: u32) -> (f32, f32) {
        let (dps, variance) = match mask & 0x7e00 {
            0x200 => (self.0[10], 0.3_f32),
            0x400 => (self.0[11], 0.2),
            0x800 => (self.0[12], 0.3),
            0x1000 => (self.0[13], 0.2),
            0x2000 => (self.0[14], 0.3),
            0x4000 => (self.0[15], 0.3),
            _ => (0, 0.3),
        };
        let (dps, variance) = (f64::from(dps), f64::from(variance));
        (
            (dps - dps * variance) as f32,
            (dps * (variance + 1.0)) as f32,
        )
    }

    /// The spell power bit `0x8000` adds (`row + 0x40`).
    pub(super) fn spell_power(&self) -> i32 {
        self.0[16]
    }
}

/// One slot's share: `budget * bonus / 10000`, a 32-bit product divided toward zero (`0x61f60f`).
pub(super) fn share(budget: i32, bonus: i32) -> i32 {
    budget.wrapping_mul(bonus) / 10_000
}

/// The level a scaling item shows (`0x61e740`): 1 for the minimum-level caller, else the link's
/// level, else the requesting unit's (never the open vendor's), else the player's; then held to
/// the item's required level and the distribution's maximum, and to at least 1.
pub(super) fn shown_level(
    v: &ItemTemplateView,
    tables: &ScalingTables,
    minimum: bool,
    link_level: i32,
    requester_level: i32,
    player_level: i32,
) -> i32 {
    let mut level = if minimum {
        1
    } else if link_level != 0 {
        link_level
    } else {
        requester_level
    };
    if level == 0 {
        level = player_level;
    }
    if let Some(d) = tables.distributions.get(&v.scaling_distribution) {
        let required = v.required_level as i32;
        if required.max(level) >= d.max_level {
            level = d.max_level;
        } else if required > level {
            level = required;
        }
    }
    level.max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row() -> ScalingValuesView {
        // Each column holds 100 plus its index, so a pick names its column.
        ScalingValuesView(std::array::from_fn(|i| 100 + i as i32))
    }

    #[test]
    fn each_budget_bit_picks_its_column_and_two_bits_pick_none() {
        let r = row();
        let picks: Vec<_> = [0x1, 0x2, 0x4, 0x8, 0x10, 0x4_0000]
            .into_iter()
            .map(|m| r.budget(m))
            .collect();
        assert_eq!(picks, [102, 103, 104, 117, 105, 118]);
        assert_eq!(r.budget(0x3), 0);
        assert_eq!(r.budget(0x8000), 0, "a bit outside the budget mask");
        // The armor and damage bits do not disturb the budget pick.
        assert_eq!(r.budget(0x8 | 0x100 | 0x200), 117);
    }

    #[test]
    fn each_armor_bit_picks_its_column() {
        let r = row();
        let picks: Vec<_> = [
            0x20, 0x40, 0x80, 0x100, 0x8_0000, 0x10_0000, 0x20_0000, 0x40_0000, 0x80_0000,
        ]
        .into_iter()
        .map(|m| r.armor(m))
        .collect();
        assert_eq!(picks, [106, 107, 108, 109, 119, 120, 121, 122, 123]);
        assert_eq!(r.armor(0x20 | 0x40), 0);
    }

    #[test]
    fn the_damage_band_is_thirty_percent_or_twenty_for_two_handers() {
        let r = ScalingValuesView(std::array::from_fn(|i| if i == 10 { 100 } else { 50 }));
        assert_eq!(r.damage(0x200), (70.0, 130.0));
        assert_eq!(r.damage(0x400), (40.0, 60.0));
        assert_eq!(r.damage(0), (0.0, 0.0));
    }

    #[test]
    fn a_share_divides_toward_zero() {
        assert_eq!(share(57, 12_345), 70);
        assert_eq!(share(-57, 12_345), -70);
        assert_eq!(share(3, 3_333), 0);
    }

    #[test]
    fn the_shown_level_is_held_between_the_requirement_and_the_maximum() {
        let mut tables = ScalingTables::default();
        tables.distributions.insert(
            7,
            ScalingDistributionView {
                max_level: 80,
                ..Default::default()
            },
        );
        let v = ItemTemplateView {
            scaling_distribution: 7,
            required_level: 1,
            ..Default::default()
        };
        let at = |min, link, req, player| shown_level(&v, &tables, min, link, req, player);
        assert_eq!(at(false, 0, 0, 34), 34, "the player's level");
        assert_eq!(at(false, 0, 12, 34), 12, "a requesting unit's level wins");
        assert_eq!(at(false, 20, 12, 34), 20, "a link's level wins");
        assert_eq!(at(true, 20, 12, 34), 1, "the minimum-level caller");
        assert_eq!(at(false, 0, 0, 85), 80, "the distribution's maximum");
        let gated = ItemTemplateView {
            required_level: 10,
            ..v.clone()
        };
        assert_eq!(shown_level(&gated, &tables, true, 0, 0, 34), 10);
        // No distribution row: the level stands, at least 1.
        let lone = ItemTemplateView {
            scaling_distribution: 9,
            ..v
        };
        assert_eq!(shown_level(&lone, &tables, false, 0, 0, 85), 85);
        assert_eq!(shown_level(&lone, &tables, false, 0, 0, 0), 1);
    }

    #[test]
    fn the_values_row_is_level_minus_one() {
        let tables = ScalingTables {
            values: vec![ScalingValuesView([1; 24]), ScalingValuesView([2; 24])],
            ..Default::default()
        };
        assert_eq!(tables.values_at(2).map(|r| r.0[0]), Some(2));
        assert_eq!(tables.values_at(0), None);
        assert_eq!(tables.values_at(3), None);
    }
}
