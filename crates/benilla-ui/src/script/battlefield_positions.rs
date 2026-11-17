//! The battleground position verbs `WorldMapFrame.lua` and `Blizzard_BattlefieldMinimap.lua` poll
//! to place teammates and the flag carriers (`MSG_BATTLEGROUND_PLAYER_POSITIONS`, `0x54b3f0`), each
//! read through the live view when called and projected onto the displayed map here.

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::number_arg;
use super::worldmap::WorldMapPoint;

/// One teammate as `GetBattlefieldPosition` reads it (`0x54c2e0`): the point and the full name.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct BattlefieldPositionLive {
    /// A held player's position, else the packet's at height 0, on the active map.
    pub at: WorldMapPoint,
    /// The name record's full name, `None` while asked.
    pub name: Option<String>,
}

/// A flag carrier as `GetBattlefieldFlagPosition` reads it (`0x54dcc0`).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct BattlefieldFlagLive {
    /// The carrier's own position (`0x5444f0`): held, or its party record's; `None` for neither.
    pub carrier: Option<WorldMapPoint>,
    /// The packet's position on the active map.
    pub packet: WorldMapPoint,
    /// `"AllianceFlag"` or `"HordeFlag"` (`0x54d010`).
    pub token: Option<&'static str>,
}

fn three(lua: &Lua, uv: (f32, f32), third: Option<&str>) -> mlua::Result<MultiValue> {
    Ok(MultiValue::from_vec(vec![
        Value::Number(f64::from(uv.0)),
        Value::Number(f64::from(uv.1)),
        match third {
            Some(s) => Value::String(lua.create_string(s)?),
            None => Value::Nil,
        },
    ]))
}

/// The reference's 1-based index, `(int)tonumber − 1`.
fn index0(n: i32) -> Option<usize> {
    usize::try_from(n.checked_sub(1)?).ok()
}

fn read<R>(f: impl FnOnce(&dyn super::LiveState) -> R) -> Option<R> {
    super::live::read_live(f)
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    g.set(
        "GetNumBattlefieldPositions",
        lua.create_function(|_, ()| {
            Ok(read(|l| l.battlefield_position_count()).unwrap_or(0) as i64)
        })?,
    )?;

    // Three values on every leg: `(0, 0, nil)` off the list; projected with no Outland hide (`0x54c4a0`).
    g.set(
        "GetBattlefieldPosition",
        lua.create_function(|lua, index: Value| {
            let index = number_arg(lua, index, "Usage: GetBattlefieldPosition(index)")?;
            let Some(row) =
                index0(index).and_then(|i| read(|l| l.battlefield_position(i)).flatten())
            else {
                return three(lua, (0.0, 0.0), None);
            };
            let uv = super::worldmap::with_map(lua, |wm| wm.project(row.at).0);
            three(lua, uv, row.name.as_deref())
        })?,
    )?;

    g.set(
        "GetNumBattlefieldFlagPositions",
        lua.create_function(|_, ()| Ok(read(|l| l.battlefield_flag_count()).unwrap_or(0) as i64))?,
    )?;

    // The carrier's blip, else on `(0, 0)` the packet's point projected (`0x54dd39`, `0x54dd81`).
    g.set(
        "GetBattlefieldFlagPosition",
        lua.create_function(|lua, index: Value| {
            let index = number_arg(lua, index, "Usage: GetBattlefieldFlagPosition(index)")?;
            let Some(flag) = index0(index).and_then(|i| read(|l| l.battlefield_flag(i)).flatten())
            else {
                return three(lua, (0.0, 0.0), None);
            };
            let uv =
                super::worldmap::with_map(lua, |wm| match flag.carrier.map(|p| wm.blip(Some(p))) {
                    Some(uv) if uv != (0.0, 0.0) => uv,
                    _ => wm.project(flag.packet).0,
                });
            three(lua, uv, flag.token)
        })?,
    )?;

    g.set(
        "GetBattlefieldMapIconScale",
        lua.create_function(|_, ()| {
            Ok(f64::from(
                read(|l| l.battlefield_icon_scale()).unwrap_or(1.0),
            ))
        })?,
    )?;

    g.set(
        "RequestBattlefieldPositions",
        lua.create_function(|_, ()| {
            read(|l| l.request_battlefield_positions());
            Ok(())
        })?,
    )?;

    // GetNumBattlefieldVehicles() (`0x54a140`) counts the vehicle list `0x549740` gathers from the
    // map's vehicle units, and GetBattlefieldVehicleInfo(i) (`0x54c4d0`) answers nothing past it.
    // benilla models no vehicle (#235), so the list is empty.
    g.set(
        "GetNumBattlefieldVehicles",
        lua.create_function(|_, ()| Ok(0))?,
    )?;
    // GetNumArenaOpponents() (`0x54a2c0`): the leading slots holding an opponent or a pet (`0x549630`), 0 without them.
    g.set(
        "GetNumArenaOpponents",
        lua.create_function(|_, ()| {
            Ok(super::live::read_live(|l| l.arena_opponent_count()).unwrap_or(0) as i64)
        })?,
    )?;
    g.set(
        "GetBattlefieldVehicleInfo",
        lua.create_function(|lua, index: Value| {
            number_arg(lua, index, "Usage: GetBattlefieldVehicleInfo(index)")?;
            Ok(MultiValue::new())
        })?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;
    use crate::script::{LiveState, UiScript};

    /// Two teammates outside the group, one carrier and a count of the requests made.
    #[derive(Default)]
    struct View {
        rows: Vec<BattlefieldPositionLive>,
        flags: Vec<BattlefieldFlagLive>,
        requests: Cell<u32>,
        arena: usize,
    }

    impl LiveState for View {
        fn unit_flags(&self, _: u64) -> Option<u32> {
            None
        }
        fn unit_health(&self, _: u64) -> Option<u32> {
            None
        }
        fn active_player(&self) -> Option<u64> {
            None
        }
        fn has_control(&self) -> bool {
            true
        }
        fn far_sight(&self) -> bool {
            false
        }
        fn battlefield_position_count(&self) -> usize {
            self.rows.len()
        }
        fn battlefield_position(&self, index: usize) -> Option<BattlefieldPositionLive> {
            self.rows.get(index).cloned()
        }
        fn battlefield_flag_count(&self) -> usize {
            self.flags.len()
        }
        fn battlefield_flag(&self, index: usize) -> Option<BattlefieldFlagLive> {
            self.flags.get(index).cloned()
        }
        fn battlefield_icon_scale(&self) -> f32 {
            1.25
        }
        fn request_battlefield_positions(&self) {
            self.requests.set(self.requests.get() + 1);
        }
        fn arena_opponent_count(&self) -> usize {
            self.arena
        }
    }

    fn view() -> View {
        View {
            rows: vec![
                BattlefieldPositionLive {
                    at: WorldMapPoint::flat(489, 1000.0, 1400.0),
                    name: Some("Probe-Realm".into()),
                },
                BattlefieldPositionLive {
                    at: WorldMapPoint::flat(489, 1100.0, 1450.0),
                    name: None,
                },
            ],
            flags: vec![BattlefieldFlagLive {
                carrier: None,
                packet: WorldMapPoint::flat(489, 1200.0, 1500.0),
                token: Some("HordeFlag"),
            }],
            ..Default::default()
        }
    }

    /// Each getter reads the view lent around its call; with none lent it answers the empty list.
    #[test]
    fn the_getters_read_the_view_at_the_call_and_answer_three_values() {
        let mut s = UiScript::new().unwrap();
        assert_eq!(
            s.eval::<i64>("return GetNumBattlefieldPositions()")
                .unwrap(),
            0
        );
        assert_eq!(s.arity("GetBattlefieldPosition(1)").unwrap(), 3);
        let view = view();
        s.with_live(&view, |s| {
            assert_eq!(
                s.eval::<i64>("return GetNumBattlefieldPositions()")
                    .unwrap(),
                2
            );
            let got = s
                .eval::<String>(
                    "local x, y, n = GetBattlefieldPosition(1) return x .. '|' .. y .. '|' .. tostring(n)",
                )
                .unwrap();
            assert_eq!(got, "0|0|Probe-Realm", "off the displayed map");
            for (i, want) in [("2", "nil"), ("'2.9'", "nil"), ("0", "nil"), ("3", "nil"), ("-1", "nil")] {
                let got = s
                    .eval::<String>(&format!(
                        "local x, y, n = GetBattlefieldPosition({i}) return tostring(n)"
                    ))
                    .unwrap();
                assert_eq!(got, want, "index {i}");
            }
            assert_eq!(
                s.eval::<i64>("return GetNumBattlefieldFlagPositions()")
                    .unwrap(),
                1
            );
            let got = s
                .eval::<String>(
                    "local x, y, t = GetBattlefieldFlagPosition(1) return x .. '|' .. y .. '|' .. t",
                )
                .unwrap();
            assert_eq!(got, "0|0|HordeFlag");
            assert!(s
                .eval::<bool>(
                    "local x, y, t = GetBattlefieldFlagPosition(2) return x == 0 and t == nil"
                )
                .unwrap());
            assert!(
                (s.eval::<f64>("return GetBattlefieldMapIconScale()")
                    .unwrap()
                    - 1.25)
                    .abs()
                    < 1e-6
            );
            s.run("RequestBattlefieldPositions() RequestBattlefieldPositions()")
                .unwrap();
        });
        assert_eq!(view.requests.get(), 2, "each call reaches the view at once");
        let err = s
            .run("GetBattlefieldPosition(nil)")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("Usage: GetBattlefieldPosition(index)"),
            "{err}"
        );
        let err = s
            .run("GetBattlefieldFlagPosition('q')")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("Usage: GetBattlefieldFlagPosition(index)"),
            "{err}"
        );
        assert_eq!(
            s.eval::<f64>("return GetBattlefieldMapIconScale()")
                .unwrap(),
            1.0
        );
    }

    /// The opponent count reads the view at the call; with none lent it answers 0, never the token push.
    #[test]
    fn the_opponent_count_reads_the_view_at_the_call() {
        let mut s = UiScript::new().unwrap();
        s.set_arena_unit_guids([1, 2, 0, 0, 0], [0; 5], 2);
        assert_eq!(s.eval::<i64>("return GetNumArenaOpponents()").unwrap(), 0);
        let view = View {
            arena: 3,
            ..Default::default()
        };
        s.with_live(&view, |s| {
            assert_eq!(s.eval::<i64>("return GetNumArenaOpponents()").unwrap(), 3);
        });
    }
}
