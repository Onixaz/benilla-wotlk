//! The zone skybox slots: which `LightSkybox.dbc` models the area-light blend asks for, and at what
//! weight, as `DayNight` builds them.
//!
//! The reference carries three `(lightSkyboxID, weight)` pairs in its blended light record
//! (`CurrentLight::m_skyboxes`). `LoadLightingBands` (`0x7ebff0`) seeds pair 0 with the row's
//! `LightParams.lightSkyboxID` at weight 1.0 and leaves the other two empty. `BlendOutputs`
//! (`0x7ec9a0`) then folds each area light's id in at that light's blend weight: onto the pair
//! already holding the id (summed, capped at 1.0) or into the first empty pair. The other pairs
//! keep their weights; nothing fades out. `SetColors` (`0x7f3230`) compacts the pairs into the
//! frame's draw list, and that list is what [`crate::LightCatalog::skybox_blend`] returns.

/// One `CurrentLight::SkyboxBlend` pair: a `LightSkybox.dbc` id (0 = empty) and its weight.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SkyboxPair {
    pub id: u32,
    pub weight: f32,
}

/// The three pairs of one blended light record (`CurrentLight::m_skyboxes`).
pub type SkyboxPairs = [SkyboxPair; 3];

/// `LoadLightingBands`' seed (`0x7ebff0`): pair 0 is the row's skybox id at weight 1.0, the other
/// two empty (the `CurrentLight` constructor, `0x7ee360`). An id of 0 still takes weight 1.0, which
/// the merge reads as an empty pair.
pub fn seed_pairs(skybox_id: u32) -> SkyboxPairs {
    let mut pairs = SkyboxPairs::default();
    pairs[0] = SkyboxPair {
        id: skybox_id,
        weight: 1.0,
    };
    pairs
}

/// `BlendOutputs`' skybox fold (`0x7ec9a0`): with a non-zero `id` and `t > 0`, add `t` to the pair
/// already holding `id` (capped at 1.0), else write `(id, t)` into the first empty pair. A full set
/// of three other ids drops it. `t` arrives clamped, as the blend weight always is.
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

/// `LightSkybox.dbc` `Flags` bit 1 (`0x1`): the model's sequence is pinned to the time of day
/// (`SyncSkyAnimWithTime`, `0x7ecf20`).
pub const SKYBOX_FLAG_SYNC_TIME: u32 = 0x1;

/// `LightSkybox.dbc` `Flags` bit 2 (`0x2`): the sky combines with the procedural one. It never
/// hides the stars, sun, moons, gradient and clouds, and a later full-weight sky does not displace
/// it (`SetColors`; `RenderSky`, `0x7f09b0`). The auroras carry it.
pub const SKYBOX_FLAG_COMBINE: u32 = 0x2;

/// One sky of the frame's draw list (`DNInfo::m_blendSkybox[i]`, `m_blendSkyboxWeight[i]`,
/// `m_blendSkyboxFlags[i]`), resolved to its model.
#[derive(Clone, Debug, PartialEq)]
pub struct SkyboxDraw {
    /// The model's chain path, spelled as [`crate::models::model_path`] spells it; empty for a
    /// record with no name, the null sky `GetOverrideSky` returns, which holds its place in the
    /// list but draws nothing.
    pub path: String,
    pub weight: f32,
    /// The record's whole `Flags` column; the reference stores only `& 2` in the draw list and
    /// reads `& 1` off the model's own copy, which is the same record's.
    pub flags: u32,
}

impl SkyboxDraw {
    /// `m_blendSkyboxFlags`: the [`SKYBOX_FLAG_COMBINE`] bit.
    pub fn combines(&self) -> bool {
        self.flags & SKYBOX_FLAG_COMBINE != 0
    }
}

/// `SetColors`' compaction (`0x7f3230`, the loop after the planet colours): walk the pairs in
/// order, skipping an empty id, a weight ≤ 0 or an id with no `LightSkybox` record. A sky above
/// 0.99 without [`SKYBOX_FLAG_COMBINE`] restarts the list at index 0, so a full-weight zone sky
/// replaces the skies before it while the ones after it still append.
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
