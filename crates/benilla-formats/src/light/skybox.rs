//! The zone skies: which `LightSkybox.dbc` models the area-light blend asks for, at what weight.
//!
//! The blended light record carries three (skybox id, weight) pairs: the band load (`0x7ebff0`)
//! seeds them, the output blend (`0x7ed4c0`) folds each area light's id in, and the colour pass
//! (`0x7f3230`) compacts them into the draw list [`crate::LightCatalog::skybox_blend`] returns.

/// One pair of the blended light record: a `LightSkybox.dbc` id (0 = empty) and its weight.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SkyboxPair {
    pub id: u32,
    pub weight: f32,
}

/// The three pairs of one blended light record.
pub type SkyboxPairs = [SkyboxPair; 3];

/// The band load's seed (`0x7ebff0`, empty pairs from `0x7ee360`): pair 0 is the row's id at 1.0.
pub fn seed_pairs(skybox_id: u32) -> SkyboxPairs {
    let mut pairs = SkyboxPairs::default();
    pairs[0] = SkyboxPair {
        id: skybox_id,
        weight: 1.0,
    };
    pairs
}

/// The output blend's skybox fold (`0x7ed4c0`): sum onto the pair holding `id` (capped at 1.0),
/// else take the first empty pair; a full set drops it.
pub fn merge_pair(pairs: &mut SkyboxPairs, id: u32, t: f32) {
    if id == 0 || t <= 0.0 {
        return;
    }
    for pair in pairs.iter_mut() {
        if pair.id == id {
            pair.weight = (pair.weight + t).min(1.0);
            return;
        }
        if pair.id == 0 {
            *pair = SkyboxPair { id, weight: t };
            return;
        }
    }
}

/// `LightSkybox.dbc` flag `0x1`: the sequence is pinned to the time of day (`0x7ecf20`).
pub const SKYBOX_FLAG_SYNC_TIME: u32 = 0x1;

/// `LightSkybox.dbc` flag `0x2`: the sky combines with the procedural one and never hides it or
/// clears the list (`0x7f3230`, `0x7f09b0`); the auroras carry it.
pub const SKYBOX_FLAG_COMBINE: u32 = 0x2;

/// One sky of the frame's draw list, resolved to its model.
#[derive(Clone, Debug, PartialEq)]
pub struct SkyboxDraw {
    /// The model's chain path; empty for an unnamed record, a null sky that holds its place.
    pub path: String,
    pub weight: f32,
    /// The record's `Flags`: the list keeps `0x2`, the model keeps `0x1`.
    pub flags: u32,
}

impl SkyboxDraw {
    /// The [`SKYBOX_FLAG_COMBINE`] bit.
    pub fn combines(&self) -> bool {
        self.flags & SKYBOX_FLAG_COMBINE != 0
    }
}

/// The colour pass's compaction (`0x7f3230`): skip empty, weightless or recordless pairs; a
/// non-combining sky above 0.99 restarts the list.
pub fn compact_pairs(
    pairs: &SkyboxPairs,
    record: impl Fn(u32) -> Option<(String, u32)>,
) -> Vec<SkyboxDraw> {
    let mut out: Vec<SkyboxDraw> = Vec::with_capacity(3);
    for pair in pairs {
        if pair.id == 0 || pair.weight <= 0.0 {
            continue;
        }
        let Some((path, flags)) = record(pair.id) else {
            continue;
        };
        if pair.weight > 0.99 && !out.is_empty() && flags & SKYBOX_FLAG_COMBINE == 0 {
            out.clear();
        }
        out.push(SkyboxDraw {
            path,
            weight: pair.weight,
            flags,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(id: u32) -> Option<(String, u32)> {
        match id {
            10 => Some(("Zone".into(), 1)),
            20 => Some(("Other".into(), 0)),
            30 => Some(("Aurora".into(), SKYBOX_FLAG_COMBINE)),
            _ => None,
        }
    }

    #[test]
    fn the_seed_takes_weight_one_even_without_a_sky() {
        assert_eq!(
            seed_pairs(0),
            [
                SkyboxPair { id: 0, weight: 1.0 },
                SkyboxPair::default(),
                SkyboxPair::default()
            ]
        );
    }

    #[test]
    fn a_skyless_base_lets_the_first_area_sky_into_pair_zero_at_its_weight() {
        // The old continents' globals name no sky: the area light's id lands in the empty pair 0.
        let mut p = seed_pairs(0);
        merge_pair(&mut p, 10, 0.25);
        assert_eq!(
            p[0],
            SkyboxPair {
                id: 10,
                weight: 0.25
            }
        );
        let draws = compact_pairs(&p, rec);
        assert_eq!(draws.len(), 1);
        assert_eq!((draws[0].path.as_str(), draws[0].weight), ("Zone", 0.25));
    }

    #[test]
    fn the_same_id_sums_and_caps_and_other_pairs_keep_their_weights() {
        let mut p = seed_pairs(20);
        merge_pair(&mut p, 10, 0.75);
        merge_pair(&mut p, 10, 0.5);
        assert_eq!(
            p[0],
            SkyboxPair {
                id: 20,
                weight: 1.0
            },
            "no fade-out of the base"
        );
        assert_eq!(
            p[1],
            SkyboxPair {
                id: 10,
                weight: 1.0
            },
            "capped at 1.0"
        );
    }

    #[test]
    fn a_zero_weight_or_a_zero_id_adds_nothing_and_a_fourth_id_is_dropped() {
        let mut p = seed_pairs(20);
        merge_pair(&mut p, 10, 0.0);
        merge_pair(&mut p, 0, 0.5);
        assert_eq!(p, seed_pairs(20));
        merge_pair(&mut p, 10, 0.5);
        merge_pair(&mut p, 30, 0.5);
        merge_pair(&mut p, 40, 0.5);
        assert_eq!(p.map(|x| x.id), [20, 10, 30]);
    }

    #[test]
    fn a_full_weight_sky_restarts_the_list_unless_it_combines() {
        // Base 20 at 1.0, then zone 10 at full weight: 10 replaces 20.
        let mut p = seed_pairs(20);
        merge_pair(&mut p, 10, 1.0);
        let d = compact_pairs(&p, rec);
        assert_eq!(
            d.iter().map(|d| d.path.as_str()).collect::<Vec<_>>(),
            ["Zone"]
        );

        // A partial zone sky draws over the base.
        let mut p = seed_pairs(20);
        merge_pair(&mut p, 10, 0.5);
        let d = compact_pairs(&p, rec);
        assert_eq!(
            d.iter().map(|d| d.path.as_str()).collect::<Vec<_>>(),
            ["Other", "Zone"]
        );

        // The aurora combines: it never clears the list, and what follows it still appends.
        let mut p = seed_pairs(20);
        merge_pair(&mut p, 30, 1.0);
        let d = compact_pairs(&p, rec);
        assert_eq!(
            d.iter().map(|d| d.path.as_str()).collect::<Vec<_>>(),
            ["Other", "Aurora"]
        );
        assert!(d[1].combines() && !d[0].combines());
    }

    #[test]
    fn an_id_with_no_record_is_skipped() {
        let mut p = seed_pairs(99);
        merge_pair(&mut p, 10, 0.5);
        let d = compact_pairs(&p, rec);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].path, "Zone");
    }
}
