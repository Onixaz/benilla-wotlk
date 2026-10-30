//! The vehicle blips (`0x581e80`, `0x580720`): every unit whose vehicle's `Vehicle.dbc` row sets
//! the battlefield-list flag, alive and within 0.8 of the view radius, draws one of seven
//! `Interface\Minimap\Vehicle-*` icons turned to its facing. A vehicle its possessor rides as a
//! group member replaces that member's blip; our own draws no blip, our arrow standing for it.

use bevy::math::Rect;
use bevy::prelude::*;

use benilla_assets::coords::bevy_to_wow;

use benilla_protocol::messages::OwnerFallback;

use crate::net::ObjectStore;
use crate::target::UNIT_FLAG_POSSESSED;
use crate::ui_pass::{UiQuad, UiQuads};
use crate::vehicle::{Vehicle, VehicleTables};

use super::{BlipCtx, BLIP_BASIS_PX, BLIP_EDGE_RATIO};

/// `Vehicle.dbc` `Flags` bit 28: the unit joins the battlefield vehicle list at post-init
/// (`0x7401a9`), the only list the minimap's vehicle pass reads.
const VEHICLE_FLAG_TRACKED: u32 = 0x1000_0000;
/// The battlefield vehicle list's capacity (`0x54aba0`).
const TRACKED_VEHICLES_MAX: usize = 40;
/// The vehicle blip's quad, `0.0375` of the screen height × 1.6667 (`0x57de36`): 48 px on the
/// 140.8-px basis.
const VEHICLE_QUAD_PX: f32 = 48.0;

/// The seven vehicle lists' art in list order, 16 to 22 (`0x5832f0`); list 20 has no arm in the
/// classifier, so its second `Ground-Unoccupied` never draws.
pub(crate) const VEHICLE_ICONS: [&str; 7] = [
    "Interface\\Minimap\\Vehicle-Ground-Unoccupied",
    "Interface\\Minimap\\Vehicle-Ground-Occupied",
    "Interface\\Minimap\\Vehicle-Air-Unoccupied",
    "Interface\\Minimap\\Vehicle-Air-Occupied",
    "Interface\\Minimap\\Vehicle-Ground-Unoccupied",
    "Interface\\Minimap\\Vehicle-Air-Horde",
    "Interface\\Minimap\\Vehicle-Air-Alliance",
];

/// A vehicle's list, counted from list 16 (`0x582cb4`): `UiLocomotionType` 0 ground and 1 air,
/// each occupied while `UNIT_FLAG_POSSESSED` is set, 3 the Horde and 4 the Alliance flier; 2 and
/// anything above 4 draw nothing.
pub(in crate::minimap) fn vehicle_list(ui_locomotion_type: u32, possessed: bool) -> Option<usize> {
    match (ui_locomotion_type, possessed) {
        (0, false) => Some(0),
        (0, true) => Some(1),
        (1, false) => Some(2),
        (1, true) => Some(3),
        (3, _) => Some(5),
        (4, _) => Some(6),
        _ => None,
    }
}

/// One tracked vehicle, as the pass reads it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::minimap) struct TrackedVehicle {
    /// WoW world position.
    pub(in crate::minimap) pos: [f32; 3],
    pub(in crate::minimap) facing: f32,
    pub(in crate::minimap) ui_locomotion_type: u32,
    pub(in crate::minimap) dead: bool,
    /// `UNIT_FLAG_POSSESSED`, the occupied icon.
    pub(in crate::minimap) possessed: bool,
    /// `UNIT_FIELD_CHARMEDBY`, else `UNIT_FIELD_CREATEDBY`, while `UNIT_FLAG_POSSESSED` is set.
    pub(in crate::minimap) possessor: Option<u64>,
}

/// The vehicle pass's verdicts for one frame.
#[derive(Debug, Default, PartialEq)]
pub(in crate::minimap) struct VehicleSelection {
    /// `(list, pos, facing)` in list order, then list-walk order.
    pub(in crate::minimap) blips: Vec<(usize, [f32; 3], f32)>,
    /// Group members a tracked vehicle shows instead of their own blip (`0x7f3f40`).
    pub(in crate::minimap) possessors: Vec<u64>,
    /// Our own possessed vehicle is in range, so our arrow keeps its full alpha though we ride.
    pub(in crate::minimap) own_in_range: bool,
}

/// The pass over the tracked vehicles (`0x582b85`): the dead skip, the 3-D cut at 0.8 of the view
/// radius, our own possessed vehicle hidden behind our arrow, the rest listed by type.
pub(in crate::minimap) fn select_vehicles(
    vehicles: &[TrackedVehicle],
    me: [f32; 3],
    self_guid: Option<u64>,
    radius_yd: f32,
) -> VehicleSelection {
    let mut sel = VehicleSelection::default();
    let cut = radius_yd * BLIP_EDGE_RATIO;
    for v in vehicles.iter().take(TRACKED_VEHICLES_MAX) {
        if let Some(p) = v.possessor {
            sel.possessors.push(p);
        }
        let d2 = (0..3).map(|i| (v.pos[i] - me[i]).powi(2)).sum::<f32>();
        if v.dead || d2 > cut * cut {
            continue;
        }
        if v.possessor.is_some() && v.possessor == self_guid {
            sel.own_in_range = true;
            continue;
        }
        if let Some(list) = vehicle_list(v.ui_locomotion_type, v.possessed) {
            sel.blips.push((list, v.pos, v.facing));
        }
    }
    // Each list draws as one batch, 16 first (`0x580720`).
    sel.blips.sort_by_key(|b| b.0);
    sel
}

/// The player arrow's alpha while we ride a seat (`0x582b60`), unless our own possessed vehicle
/// is in range.
pub(in crate::minimap) const RIDING_ARROW_ALPHA: f32 = 170.0 / 255.0;

/// The tracked vehicles among the streamed units.
pub(in crate::minimap) fn tracked_vehicles(
    units: &Query<(&Vehicle, &GlobalTransform, &ObjectStore)>,
    tables: &VehicleTables,
    out: &mut Vec<TrackedVehicle>,
) {
    out.clear();
    for (vehicle, tf, store) in units {
        let Some(rec) = tables
            .vehicle(vehicle.id)
            .filter(|r| r.flags & VEHICLE_FLAG_TRACKED != 0)
        else {
            continue;
        };
        let fields = &store.0;
        let possessed = fields.unit_flags() & UNIT_FLAG_POSSESSED != 0;
        let possessor = possessed
            .then(|| fields.unit_owner(OwnerFallback::CreatedBy))
            .flatten();
        out.push(TrackedVehicle {
            pos: bevy_to_wow(tf.translation()),
            facing: tf.compute_transform().rotation.to_euler(EulerRot::YXZ).0,
            ui_locomotion_type: rec.ui_locomotion_type,
            // `0x716490`: health at or below 0.
            dead: fields.unit_health().is_some_and(|h| h == 0),
            possessed,
            possessor,
        });
    }
}

/// Draw the selected vehicle blips. `0x580720` samples a uv square turned about its centre with
/// its corners at radius 1, so the art fills `1/√2` of the quad, pointing along the facing.
pub(in crate::minimap) fn emit_vehicle_blips(
    ctx: &BlipCtx,
    sel: &VehicleSelection,
    icons: &[Option<Handle<Image>>; 7],
    quads: &mut UiQuads,
) {
    let px = ctx.side * (VEHICLE_QUAD_PX / BLIP_BASIS_PX) * std::f32::consts::FRAC_1_SQRT_2;
    for &(list, pos, facing) in &sel.blips {
        let Some(tex) = &icons[list] else {
            continue;
        };
        quads.overlays.push(UiQuad {
            rect: Rect::from_center_size(ctx.center + ctx.offset(pos), Vec2::splat(px)),
            z_key: ctx.z,
            texture: Some(tex.clone()),
            color: [1.0, 1.0, 1.0, ctx.alpha],
            // Deviates: `0x5808c6` leaves it unturned under `rotateMinimap`, pointing wrong on a turned map.
            rotation: ctx.turn - facing,
            ..default()
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: f32, kind: u32, possessor: Option<u64>) -> TrackedVehicle {
        TrackedVehicle {
            pos: [x, 0.0, 0.0],
            facing: 0.0,
            ui_locomotion_type: kind,
            dead: false,
            possessed: possessor.is_some(),
            possessor,
        }
    }

    #[test]
    fn the_list_is_the_locomotion_type_and_possession() {
        assert_eq!(vehicle_list(0, false), Some(0), "Ground-Unoccupied");
        assert_eq!(vehicle_list(0, true), Some(1), "Ground-Occupied");
        assert_eq!(vehicle_list(1, false), Some(2), "Air-Unoccupied");
        assert_eq!(vehicle_list(1, true), Some(3), "Air-Occupied");
        assert_eq!(vehicle_list(2, false), None, "type 2 has no arm");
        assert_eq!(vehicle_list(3, true), Some(5), "Air-Horde, occupied or not");
        assert_eq!(vehicle_list(4, false), Some(6), "Air-Alliance");
        assert_eq!(vehicle_list(5, false), None);
        assert!(VEHICLE_ICONS[5].ends_with("Air-Horde") && VEHICLE_ICONS[6].ends_with("Alliance"));
    }

    #[test]
    fn the_pass_cuts_in_three_d_skips_the_dead_and_sorts_by_list() {
        let mut dead = v(10.0, 0, None);
        dead.dead = true;
        let mut high = v(0.0, 0, None);
        high.pos[2] = 90.0; // 2-D at the centre, 3-D beyond 0.8 × 100
        let units = [
            v(10.0, 1, Some(7)),
            v(20.0, 0, None),
            dead,
            high,
            v(85.0, 0, None),
        ];
        let sel = select_vehicles(&units, [0.0; 3], Some(1), 100.0);
        let lists: Vec<usize> = sel.blips.iter().map(|b| b.0).collect();
        assert_eq!(
            lists,
            [0, 3],
            "ground before the occupied flier; dead, high, far dropped"
        );
        assert_eq!(sel.possessors, [7], "a possessor is known out of range too");
        assert!(!sel.own_in_range);
    }

    #[test]
    fn our_own_possessed_vehicle_hides_behind_the_arrow() {
        let sel = select_vehicles(&[v(5.0, 1, Some(1))], [0.0; 3], Some(1), 100.0);
        assert!(sel.blips.is_empty());
        assert!(sel.own_in_range);
        assert_eq!(sel.possessors, [1]);
    }

    #[test]
    fn the_list_holds_forty() {
        let units = vec![v(1.0, 0, None); 41];
        let sel = select_vehicles(&units, [0.0; 3], None, 100.0);
        assert_eq!(sel.blips.len(), TRACKED_VEHICLES_MAX);
    }
}
