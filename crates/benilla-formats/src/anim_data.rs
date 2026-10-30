//! `AnimationData.dbc`, the per-animation policy: 506 rows of `ID`, `Name`, `WeaponFlags`,
//! `BodyFlags`, `Flags`, `Fallback`, `BehaviorID` and `BehaviorTier` in 12340. The sheath reconcile
//! (on every animation start) reads `WeaponFlags` at row `+0x8` for the requested id, not a model's
//! substitute: `4` and `0x10` stow, `0x20` draws melee, and 12340 sets no other bit. `Fallback` is
//! walked for an id a model does not author (`0x826350`); a 3.3.5a model bakes no
//! `PlayableAnimationLookup` from it.

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, str_at, u32_at};
use crate::Chain;

/// One animation's policy row.
#[derive(Clone, Copy)]
pub struct AnimEntry {
    /// The sheath-reconcile bits (col 2).
    pub weapon_flags: u32,
    /// The substitute animation id when a model lacks this clip (`0` = Stand), col 5; col 6,
    /// `BehaviorID`, is mostly the row's own id.
    pub fallback: u16,
    /// The row's Flags (col 4): `0x10` reverses and `0x20` holds a clip the fallback walk passes
    /// through (`0x826429`).
    pub flags: u32,
    /// The BehaviorID (col 6): the base animation a tiered row stands in for, the row's own id on
    /// the ground tier.
    pub behavior: u16,
    /// The BehaviorTier (col 7): 0 ground, 3 fly; the unit's anim tier picks the row (`0x717540`).
    pub tier: u8,
}

/// `AnimationData.dbc` id → its policy row.
pub struct AnimDataCatalog {
    rows: HashMap<u16, AnimEntry>,
    /// The Name column (col 1, `"Attack1H"`) for debug readouts, kept apart so [`AnimEntry`]
    /// stays `Copy`.
    names: HashMap<u16, String>,
    /// `(behavior, tier)` → the lowest id carrying them, the tier scan's first hit (`0x7175c1`).
    by_behavior: HashMap<(u16, u8), u16>,
}

impl AnimDataCatalog {
    /// A catalog of synthetic rows with no names, for tests without an `AnimationData.dbc`.
    pub fn from_rows(rows: impl IntoIterator<Item = (u16, AnimEntry)>) -> Self {
        Self::new(rows.into_iter().collect(), HashMap::new())
    }

    fn new(rows: HashMap<u16, AnimEntry>, names: HashMap<u16, String>) -> Self {
        let mut by_behavior: HashMap<(u16, u8), u16> = HashMap::new();
        for (&id, r) in &rows {
            by_behavior
                .entry((r.behavior, r.tier))
                .and_modify(|e| *e = (*e).min(id))
                .or_insert(id);
        }
        Self {
            rows,
            names,
            by_behavior,
        }
    }

    /// The WeaponFlags for an animation id, `0` (no policy) for an unknown one.
    pub fn weapon_flags(&self, id: u16) -> u32 {
        self.rows.get(&id).map_or(0, |r| r.weapon_flags)
    }

    /// The `Fallback` column as stored, Stand and a self-fallback included; `None` for no row.
    pub fn fallback(&self, id: u16) -> Option<u16> {
        self.rows.get(&id).map(|r| r.fallback)
    }

    /// The row's `Fallback` and Flags together, the fallback walk's step; `None` for no row.
    pub fn fallback_step(&self, id: u16) -> Option<(u16, u32)> {
        self.rows.get(&id).map(|r| (r.fallback, r.flags))
    }

    /// The row's BehaviorID; `None` for no row.
    pub fn behavior(&self, id: u16) -> Option<u16> {
        self.rows.get(&id).map(|r| r.behavior)
    }

    /// The row's BehaviorTier; `None` for no row.
    pub fn tier(&self, id: u16) -> Option<u8> {
        self.rows.get(&id).map(|r| r.tier)
    }

    /// The first row, in id order, standing in for `behavior` on `tier` (`0x7175c1`).
    pub fn find_by_behavior_and_tier(&self, behavior: u16, tier: u8) -> Option<u16> {
        self.by_behavior.get(&(behavior, tier)).copied()
    }

    /// The row's Name (`"Run"`); a [`Self::from_rows`] catalog has none.
    pub fn name(&self, id: u16) -> Option<&str> {
        self.names.get(&id).map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

pub(crate) fn schema() -> Schema {
    let mut s = Schema::new("AnimationData");
    for i in 0..8 {
        let ty = if i == 1 {
            FieldType::String
        } else {
            FieldType::UInt32
        };
        s.add_field(SchemaField::new(format!("f{i}"), ty));
    }
    s
}

/// Read `AnimationData.dbc` off the patch chain.
pub fn load_anim_data_catalog(chain: &mut Chain) -> Result<AnimDataCatalog> {
    let bytes = chain
        .read_file("DBFilesClient\\AnimationData.dbc")
        .context("reading AnimationData.dbc")?;
    let rs = parse(&bytes, schema(), "AnimationData")?;
    let mut rows = HashMap::with_capacity(rs.records().len());
    let mut names = HashMap::with_capacity(rs.records().len());
    for r in rs.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        rows.insert(
            id as u16,
            AnimEntry {
                weapon_flags: u32_at(r, 2).unwrap_or(0),
                fallback: u32_at(r, 5).unwrap_or(0) as u16,
                flags: u32_at(r, 4).unwrap_or(0),
                behavior: u32_at(r, 6).unwrap_or(0) as u16,
                tier: u32_at(r, 7).unwrap_or(0) as u8,
            },
        );
        if let Some(name) = str_at(&rs, r, 1).filter(|n| !n.is_empty()) {
            names.insert(id as u16, name);
        }
    }
    Ok(AnimDataCatalog::new(rows, names))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The 12340 table: WeaponFlags within the reconcile's three bits, spot rows, fallback chains.
    #[test]
    fn real_animation_data_decodes() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_anim_data_catalog(&mut chain).expect("load AnimationData");
        assert_eq!(cat.len(), 506);

        for (&id, row) in &cat.rows {
            assert!(
                matches!(row.weapon_flags, 0 | 4 | 0x10 | 0x14 | 0x20),
                "anim {id}: unexpected WeaponFlags {:#x}",
                row.weapon_flags
            );
        }

        assert_eq!(cat.weapon_flags(42), 4, "Swim force-stows");
        assert_eq!(cat.weapon_flags(91), 4, "Mount force-stows");
        assert_eq!(cat.weapon_flags(102), 4, "SitChairLow force-stows");
        assert_eq!(cat.weapon_flags(60), 0x10, "EmoteTalk force-stows");
        assert_eq!(
            cat.weapon_flags(16),
            0x10,
            "AttackUnarmed needs empty hands"
        );
        assert_eq!(cat.weapon_flags(96), 0x10, "SitGroundDown force-stows");
        assert_eq!(cat.weapon_flags(17), 0x20, "Attack1H force-draws");
        assert_eq!(cat.weapon_flags(26), 0x20, "Ready1H force-draws");
        assert_eq!(cat.weapon_flags(133), 0x20, "FishingCast force-draws");
        assert_eq!(cat.weapon_flags(0), 0, "Stand carries no policy");

        // The fallback walk's direction bits and the tier index.
        assert_eq!(
            cat.fallback_step(13),
            Some((4, 0x11)),
            "Walkbackwards reverses Walk"
        );
        assert_eq!(
            cat.fallback_step(97).map(|s| s.1 & 0x30),
            Some(0x20),
            "SitGround holds"
        );
        assert_eq!(
            cat.find_by_behavior_and_tier(0, 0),
            Some(0),
            "Stand on the ground"
        );
        assert_eq!(
            cat.find_by_behavior_and_tier(0, 3),
            Some(229),
            "FlyStand on the fly tier"
        );
        assert_eq!((cat.behavior(229), cat.tier(229)), (Some(0), Some(3)));

        // The ranged-handling trio the reconcile exempts.
        for id in [105, 106, 112] {
            assert_eq!(cat.weapon_flags(id), 0, "Load* anims carry no policy");
        }

        assert_eq!(cat.name(0), Some("Stand"));
        assert_eq!(cat.name(5), Some("Run"));
        assert_eq!(cat.name(17), Some("Attack1H"));
        assert_eq!(cat.name(505), Some("FlyCarried2H"));
        assert_eq!(cat.name(9999), None, "unknown id has no name");

        assert_eq!(cat.fallback(18), Some(17), "Attack2H → Attack1H");
        assert_eq!(cat.fallback(17), Some(16), "Attack1H → AttackUnarmed");
        assert_eq!(cat.fallback(143), Some(5), "Sprint → Run");
        assert_eq!(cat.fallback(187), Some(5), "JumpLandRun → Run");
        assert_eq!(cat.fallback(100), Some(99), "Sleep → SleepDown");
        assert_eq!(cat.fallback(4), Some(0), "Walk → Stand");
        assert_eq!(cat.fallback(6), Some(1), "Dead → Death, the corpse pose");
        assert_eq!(cat.fallback(132), Some(131), "Drowned → Drown");
        assert_eq!(cat.fallback(9999), None, "no row");
    }
}
