//! `PetPersonality.dbc`, behind a hunter pet's happiness readout. `GetPetHappiness` buckets on its
//! own, counting the row's thresholds the happiness power (`UNIT_FIELD_POWER5`) meets. 12340's row
//! is `ID`, the name over sixteen locales and a mask, then the threshold and damage triples; 3.0
//! dropped pet loyalty, so the loyalty-rate triple and `PetLoyalty.dbc` are gone from the install.

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::chain::Chain;
use crate::dbc::{f32_at, parse, u32_at};

const PET_PERSONALITY: &str = "DBFilesClient\\PetPersonality.dbc";

/// `PetPersonality.dbc`'s column count, which `benilla-dbc` checks against the header.
const PERSONALITY_FIELDS: usize = 24;
/// The enUS `Name` column.
const NAME_FIELD: usize = 1;

const THRESHOLD_FIELD: usize = 18;
const DAMAGE_FIELD: usize = 21;

/// The personality the client falls back to when the id is out of range or its row is missing,
/// "Personality: Standard".
pub const FALLBACK_PERSONALITY: u32 = 1;

/// One `PetPersonality` row's triples, read 1-based: bucket 1 reads slot 0, bucket 0 reads none.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PetPersonality {
    /// The three ascending happiness thresholds the raw power is counted against.
    pub thresholds: [u32; 3],
    /// Damage multiplier per bucket as stored (`0.75` is 75%).
    pub damage: [f32; 3],
}

/// What `GetPetHappiness` answers: the bucket plus the damage figure it selects.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PetHappiness {
    /// `0..=3`. `0` is a number, not the failure case: the client pushes `0` with the failure's
    /// tail, and `PetFrame.lua`, with no branch for it, keeps the icon's old texcoords.
    pub bucket: u32,
    /// The damage percentage, times the client's `100.0f`; `100.0` for bucket 0.
    pub damage_percentage: f32,
}

impl PetPersonality {
    /// Bucket a raw happiness power and pick the row's damage figure.
    pub fn happiness(&self, raw: u32) -> PetHappiness {
        let bucket = self.thresholds.iter().take_while(|&&t| raw >= t).count();
        // Bucket 0 takes the gate-failure tail instead of indexing.
        let Some(i) = bucket.checked_sub(1) else {
            return PetHappiness {
                bucket: 0,
                damage_percentage: 100.0,
            };
        };
        PetHappiness {
            bucket: bucket as u32,
            damage_percentage: self.damage[i] * 100.0,
        }
    }
}

/// `PetPersonality.dbc`, by id.
pub struct PetPersonalities(HashMap<u32, PetPersonality>);

impl PetPersonalities {
    /// The row for personality `id`, or the client's fallback row. The reference keys this table by
    /// `[[pet+0xb30]+0x24]`, a field of the cached creature-query record whose wire source is
    /// untraced, so callers pass `None` and get row 1.
    pub fn for_pet(&self, id: Option<u32>) -> Option<&PetPersonality> {
        id.and_then(|i| self.0.get(&i))
            .or_else(|| self.0.get(&FALLBACK_PERSONALITY))
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

pub(crate) fn personality_schema() -> Schema {
    let mut s = Schema::new("PetPersonality");
    for i in 0..PERSONALITY_FIELDS {
        let ty = match i {
            NAME_FIELD => FieldType::String,
            i if (DAMAGE_FIELD..DAMAGE_FIELD + 3).contains(&i) => FieldType::Float32,
            // ID, the rest of the name block, and the thresholds, compared as integers.
            _ => FieldType::UInt32,
        };
        s.add_field(SchemaField::new(format!("F{i}"), ty));
    }
    s
}

/// Load `PetPersonality.dbc` from the patch chain.
pub fn load_pet_personalities(chain: &mut Chain) -> Result<PetPersonalities> {
    let bytes = chain
        .read_file(PET_PERSONALITY)
        .with_context(|| format!("reading {PET_PERSONALITY}"))?;
    let rs = parse(&bytes, personality_schema(), "PetPersonality.dbc")?;
    let mut by_id = HashMap::new();
    for r in rs.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        let triple_u32 = |base: usize| [0, 1, 2].map(|i| u32_at(r, base + i).unwrap_or(0));
        let triple_f32 = |base: usize| [0, 1, 2].map(|i| f32_at(r, base + i).unwrap_or(0.0));
        by_id.insert(
            id,
            PetPersonality {
                thresholds: triple_u32(THRESHOLD_FIELD),
                damage: triple_f32(DAMAGE_FIELD),
            },
        );
    }
    Ok(PetPersonalities(by_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chain() -> Option<crate::chain::Chain> {
        let data = crate::wow_data_or_skip!(None);
        Some(crate::open_chain(&data).expect("open chain"))
    }

    #[test]
    fn the_real_personality_rows_carry_their_happiness_numbers() {
        let Some(mut chain) = chain() else { return };
        let t = load_pet_personalities(&mut chain).expect("load PetPersonality.dbc");
        assert_eq!(
            t.len(),
            2,
            "12340 ships exactly two personalities, ids 1 and 3"
        );

        let one = t.for_pet(Some(1)).expect("id 1");
        assert_eq!(one.thresholds, [0, 333_000, 666_000]);
        assert_eq!(one.damage, [0.75, 1.0, 1.25]);

        let three = t.for_pet(Some(3)).expect("id 3");
        assert_eq!(three.thresholds, [0, 250_000, 750_000]);

        // The client's fallback, the path every caller takes, lands on row 1.
        assert_eq!(t.for_pet(None), t.for_pet(Some(1)));
        assert_eq!(t.for_pet(Some(999)), t.for_pet(Some(1)));
    }

    #[test]
    fn the_bucket_loop_counts_thresholds_met() {
        let Some(mut chain) = chain() else { return };
        let t = load_pet_personalities(&mut chain).expect("load");
        let p = *t.for_pet(None).expect("fallback row");

        // Happiness runs 0..1_000_000 in thirds, so every reachable value buckets 1..3.
        for (raw, bucket, dmg) in [
            (0, 1, 75.0),
            (332_999, 1, 75.0),
            (333_000, 2, 100.0),
            (665_999, 2, 100.0),
            (666_000, 3, 125.0),
            (1_000_000, 3, 125.0),
        ] {
            let h = p.happiness(raw);
            assert_eq!((h.bucket, h.damage_percentage), (bucket, dmg), "raw {raw}");
        }
    }

    /// Bucket 0 is reachable only through a row whose first threshold is above the raw value.
    #[test]
    fn bucket_zero_is_a_number_not_a_failure() {
        let p = PetPersonality {
            thresholds: [10, 20, 30],
            damage: [0.75, 1.0, 1.25],
        };
        let h = p.happiness(9);
        assert_eq!(h.bucket, 0);
        assert_eq!(h.damage_percentage, 100.0);
    }
}
