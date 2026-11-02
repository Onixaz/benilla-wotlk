//! The quest points of interest: `CMSG_QUEST_POI_QUERY` (483), which the quest log update sends for
//! the quests it holds no POI for (`0x5e6cd1`), and `SMSG_QUEST_POI_QUERY_RESPONSE` (484), read by
//! the quest log's POI response handler (`0x5e7370`) into the 25-slot store.

use std::io;

use crate::wire::{read_i32_le, read_u32_le};

/// One quest's POI regions as the response carries them.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct QuestPoi {
    pub quest_id: u32,
    pub blobs: Vec<QuestPoiBlob>,
}

/// One region: seven dwords then the outline (`0x5e7466`-`0x5e749d`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct QuestPoiBlob {
    /// Dword 0, the region's id.
    pub id: u32,
    /// Dword 1, the leaderboard objective the region serves, -1 for the turn-in.
    pub objective: i32,
    /// Dword 2, the `Map.dbc` id.
    pub map: u32,
    /// Dword 3, the `WorldMapArea.dbc` row.
    pub area: u32,
    /// Dword 4, the `DungeonMap` floor.
    pub floor: u32,
    /// Dword 5, record `+0x30`: the icon placement's priority, the lowest winning (`0x5e2eb0`).
    pub priority: u32,
    /// Dword 6, record `+0x2c`: bit `0x4` binds the region to its own `area` (`0x5e5ba1`).
    pub flags: u32,
    /// The outline in world `(x, y)` yards.
    pub points: Vec<(i32, i32)>,
}

/// Body of `CMSG_QUEST_POI_QUERY`: a count, then each quest id.
pub fn quest_poi_query(quests: &[u32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + quests.len() * 4);
    out.extend_from_slice(&(quests.len() as u32).to_le_bytes());
    for q in quests {
        out.extend_from_slice(&q.to_le_bytes());
    }
    out
}

/// Read `SMSG_QUEST_POI_QUERY_RESPONSE` in `0x5e7370`'s order.
pub(in crate::messages) fn read_quest_poi_query_response(
    r: &mut &[u8],
) -> io::Result<Vec<QuestPoi>> {
    let count = read_u32_le(r)?;
    let mut out = Vec::new();
    for _ in 0..count {
        let quest_id = read_u32_le(r)?;
        let n = read_u32_le(r)?;
        let mut blobs = Vec::new();
        for _ in 0..n {
            let mut b = QuestPoiBlob {
                id: read_u32_le(r)?,
                objective: read_i32_le(r)?,
                map: read_u32_le(r)?,
                area: read_u32_le(r)?,
                floor: read_u32_le(r)?,
                priority: read_u32_le(r)?,
                flags: read_u32_le(r)?,
                points: Vec::new(),
            };
            let points = read_u32_le(r)?;
            for _ in 0..points {
                b.points.push((read_i32_le(r)?, read_i32_le(r)?));
            }
            blobs.push(b);
        }
        out.push(QuestPoi { quest_id, blobs });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_query_is_a_count_then_the_ids() {
        assert_eq!(
            quest_poi_query(&[7, 0x0102]),
            vec![2, 0, 0, 0, 7, 0, 0, 0, 2, 1, 0, 0]
        );
    }

    #[test]
    fn the_response_reads_regions_and_their_outlines() {
        let mut body = Vec::new();
        let mut put = |v: i32| body.extend_from_slice(&v.to_le_bytes());
        put(1); // one quest
        put(7);
        put(1); // one region
        for v in [0, -1, 0, 12, 0, 1, 4] {
            put(v);
        }
        put(2);
        for v in [-8900, -120, -8910, -130] {
            put(v);
        }
        let mut r = &body[..];
        let pois = read_quest_poi_query_response(&mut r).unwrap();
        assert!(r.is_empty());
        assert_eq!(
            pois,
            vec![QuestPoi {
                quest_id: 7,
                blobs: vec![QuestPoiBlob {
                    id: 0,
                    objective: -1,
                    map: 0,
                    area: 12,
                    floor: 0,
                    priority: 1,
                    flags: 4,
                    points: vec![(-8900, -120), (-8910, -130)],
                }],
            }]
        );
    }
}
