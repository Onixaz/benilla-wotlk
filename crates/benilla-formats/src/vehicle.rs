//! The vehicle tables: `Vehicle.dbc` (the vehicle's seats, aim limits and UI indicator,
//! `0x757fa0`), `VehicleSeat.dbc` (a seat's attachment, transitions, flags and camera,
//! `0x756ec0`), and the seat indicator, `VehicleUIIndicator.dbc` with its `VehicleUIIndSeat.dbc`
//! seats (`0x614d90`).

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{f32_at, i32_at, parse, str_at, u32_at};
use crate::Chain;

const VEHICLE: &str = "DBFilesClient\\Vehicle.dbc";
const VEHICLE_SEAT: &str = "DBFilesClient\\VehicleSeat.dbc";
const VEHICLE_UI_INDICATOR: &str = "DBFilesClient\\VehicleUIIndicator.dbc";
const VEHICLE_UI_IND_SEAT: &str = "DBFilesClient\\VehicleUIIndSeat.dbc";
const OVERRIDE_SPELL_DATA: &str = "DBFilesClient\\OverrideSpellData.dbc";

/// An `OverrideSpellData.dbc` row's spell slots.
pub const OVERRIDE_SPELLS: usize = 10;

/// One `OverrideSpellData.dbc` row: the ten spells an override bar shows (`0x5abb40`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OverrideSpellsRec {
    pub id: u32,
    pub spells: [u32; OVERRIDE_SPELLS],
    pub flags: u32,
}

/// A vehicle's seat slots (`Vehicle.dbc` columns 6-13).
pub const VEHICLE_SEATS: usize = 8;

/// One `Vehicle.dbc` row, the columns the client reads.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct VehicleRec {
    pub id: u32,
    /// Column 1: `0x10` trajectory pitch, `0x40` pitch limits, `0x400` aim angle, `0x800` aim
    /// power, `0x100000` power display, `0x10000000` battlefield vehicle list.
    pub flags: u32,
    pub turn_speed: f32,
    pub pitch_speed: f32,
    pub pitch_min: f32,
    pub pitch_max: f32,
    /// `VehicleSeat` ids, 0 for an unused slot.
    pub seat_ids: [u32; VEHICLE_SEATS],
    pub mouse_look_offset_pitch: f32,
    pub camera_fade_dist_scalar_min: f32,
    pub camera_fade_dist_scalar_max: f32,
    pub camera_pitch_offset: f32,
    pub facing_limit_right: f32,
    pub facing_limit_left: f32,
    pub camera_yaw_offset: f32,
    pub ui_locomotion_type: u32,
    /// `VehicleUIIndicator` id, which `PLAYER_GAINS_VEHICLE_DATA` and the enter events carry.
    pub ui_indicator_id: u32,
    pub power_display_ids: [u32; 3],
}

/// One `VehicleSeat.dbc` row, the columns the client reads.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct VehicleSeatRec {
    pub id: u32,
    /// Column 1: `0x800` control seat, `0x2000000` can exit, `0x4000000` can switch,
    /// `0x20000000` has vehicle UI, among others.
    pub flags: u32,
    /// Index into the client's seat attachment table (`0xa2d3f0`).
    pub attachment_id: i32,
    pub attachment_offset: [f32; 3],
    pub enter_pre_delay: f32,
    pub enter_speed: f32,
    pub enter_gravity: f32,
    pub enter_min_duration: f32,
    pub enter_max_duration: f32,
    pub enter_min_arc_height: f32,
    pub enter_max_arc_height: f32,
    /// Enter start and loop, ride start and loop, ride upper start and loop (columns 13-18).
    pub passenger_anims: [i32; 6],
    pub exit_pre_delay: f32,
    pub exit_speed: f32,
    pub exit_gravity: f32,
    pub exit_min_duration: f32,
    pub exit_max_duration: f32,
    pub exit_min_arc_height: f32,
    pub exit_max_arc_height: f32,
    /// Exit start, loop and end (columns 26-28).
    pub exit_anims: [i32; 3],
    pub passenger_yaw: f32,
    pub passenger_pitch: f32,
    pub passenger_roll: f32,
    pub passenger_attachment_id: i32,
    /// The vehicle's enter, exit and ride-loop anims (columns 33-35).
    pub vehicle_anims: [i32; 3],
    /// Their bones (columns 36-38).
    pub vehicle_anim_bones: [i32; 3],
    pub vehicle_enter_anim_delay: f32,
    pub vehicle_exit_anim_delay: f32,
    /// Column 41: whether the seat shows the vehicle's abilities (`0x5d4ad0`).
    pub vehicle_ability_display: i32,
    /// `SoundEntries` ids the enter and exit events name (columns 42, 43).
    pub enter_ui_sound_id: u32,
    pub exit_ui_sound_id: u32,
    /// Column 44: 0 "Natural", 1 "Mechanical".
    pub ui_skin: i32,
    /// Column 45: `0x8` raid UI targets the vehicle, `0x20` can eject, `0x100000` exit on cast.
    pub flags_b: u32,
    pub camera_entering_delay: f32,
    pub camera_entering_duration: f32,
    pub camera_exiting_delay: f32,
    pub camera_exiting_duration: f32,
    pub camera_offset: [f32; 3],
    pub camera_pos_chase_rate: f32,
    pub camera_facing_chase_rate: f32,
    pub camera_entering_zoom: f32,
    pub camera_seat_zoom_min: f32,
    pub camera_seat_zoom_max: f32,
}

/// One seat of a seat indicator (`VehicleUIIndSeat.dbc`).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct IndicatorSeat {
    pub virtual_seat_index: u32,
    pub x: f32,
    pub y: f32,
}

/// A seat indicator: its background and its seats in id order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct VehicleUiIndicator {
    pub background_texture: String,
    pub seats: Vec<IndicatorSeat>,
}

/// The four tables, by id.
#[derive(Debug, Clone, Default)]
pub struct VehicleCatalog {
    pub vehicles: HashMap<u32, VehicleRec>,
    pub seats: HashMap<u32, VehicleSeatRec>,
    pub indicators: HashMap<u32, VehicleUiIndicator>,
    pub override_spells: HashMap<u32, OverrideSpellsRec>,
}

impl VehicleCatalog {
    pub fn vehicle(&self, id: u32) -> Option<&VehicleRec> {
        self.vehicles.get(&id)
    }

    pub fn seat(&self, id: u32) -> Option<&VehicleSeatRec> {
        self.seats.get(&id)
    }

    /// `0x756ec0`: seat slot `slot` of vehicle `vehicle`, `None` past the eight slots or for an
    /// unused or unknown one.
    pub fn vehicle_seat(&self, vehicle: &VehicleRec, slot: usize) -> Option<&VehicleSeatRec> {
        let id = *vehicle.seat_ids.get(slot)?;
        (id != 0).then(|| self.seats.get(&id)).flatten()
    }

    pub fn indicator(&self, id: u32) -> Option<&VehicleUiIndicator> {
        self.indicators.get(&id)
    }

    pub fn is_empty(&self) -> bool {
        self.vehicles.is_empty()
    }
}

fn schema(name: &str, fields: &[(&str, FieldType)]) -> Schema {
    let mut s = Schema::new(name);
    for &(field, ty) in fields {
        s.add_field(SchemaField::new(field, ty));
    }
    s
}

pub(crate) fn vehicle_schema() -> Schema {
    use FieldType::{Float32 as F, String as S, UInt32 as U};
    let mut f: Vec<(&str, FieldType)> = vec![
        ("ID", U),
        ("Flags", U),
        ("TurnSpeed", F),
        ("PitchSpeed", F),
        ("PitchMin", F),
        ("PitchMax", F),
    ];
    f.extend(std::iter::repeat_n(("SeatID", U), VEHICLE_SEATS));
    f.extend([
        ("MouseLookOffsetPitch", F),
        ("CameraFadeDistScalarMin", F),
        ("CameraFadeDistScalarMax", F),
        ("CameraPitchOffset", F),
        ("FacingLimitRight", F),
        ("FacingLimitLeft", F),
        ("MsslTrgtTurnLingering", F),
        ("MsslTrgtPitchLingering", F),
        ("MsslTrgtMouseLingering", F),
        ("MsslTrgtEndOpacity", F),
        ("MsslTrgtArcSpeed", F),
        ("MsslTrgtArcRepeat", F),
        ("MsslTrgtArcWidth", F),
        ("MsslTrgtImpactRadius0", F),
        ("MsslTrgtImpactRadius1", F),
        ("MsslTrgtArcTexture", S),
        ("MsslTrgtImpactTexture", S),
        ("MsslTrgtImpactModel0", S),
        ("MsslTrgtImpactModel1", S),
        ("CameraYawOffset", F),
        ("UiLocomotionType", U),
        ("MsslTrgtImpactTexRadius", F),
        ("VehicleUIIndicatorID", U),
        ("PowerDisplayID0", U),
        ("PowerDisplayID1", U),
        ("PowerDisplayID2", U),
    ]);
    schema("Vehicle", &f)
}

pub(crate) fn vehicle_seat_schema() -> Schema {
    use FieldType::{Float32 as F, Int32 as I, UInt32 as U};
    let names: [(&str, FieldType); 58] = [
        ("ID", U),
        ("Flags", U),
        ("AttachmentID", I),
        ("AttachmentOffsetX", F),
        ("AttachmentOffsetY", F),
        ("AttachmentOffsetZ", F),
        ("EnterPreDelay", F),
        ("EnterSpeed", F),
        ("EnterGravity", F),
        ("EnterMinDuration", F),
        ("EnterMaxDuration", F),
        ("EnterMinArcHeight", F),
        ("EnterMaxArcHeight", F),
        ("EnterAnimStart", I),
        ("EnterAnimLoop", I),
        ("RideAnimStart", I),
        ("RideAnimLoop", I),
        ("RideUpperAnimStart", I),
        ("RideUpperAnimLoop", I),
        ("ExitPreDelay", F),
        ("ExitSpeed", F),
        ("ExitGravity", F),
        ("ExitMinDuration", F),
        ("ExitMaxDuration", F),
        ("ExitMinArcHeight", F),
        ("ExitMaxArcHeight", F),
        ("ExitAnimStart", I),
        ("ExitAnimLoop", I),
        ("ExitAnimEnd", I),
        ("PassengerYaw", F),
        ("PassengerPitch", F),
        ("PassengerRoll", F),
        ("PassengerAttachmentID", I),
        ("VehicleEnterAnim", I),
        ("VehicleExitAnim", I),
        ("VehicleRideAnimLoop", I),
        ("VehicleEnterAnimBone", I),
        ("VehicleExitAnimBone", I),
        ("VehicleRideAnimLoopBone", I),
        ("VehicleEnterAnimDelay", F),
        ("VehicleExitAnimDelay", F),
        ("VehicleAbilityDisplay", I),
        ("EnterUISoundID", U),
        ("ExitUISoundID", U),
        ("UiSkin", I),
        ("FlagsB", U),
        ("CameraEnteringDelay", F),
        ("CameraEnteringDuration", F),
        ("CameraExitingDelay", F),
        ("CameraExitingDuration", F),
        ("CameraOffsetX", F),
        ("CameraOffsetY", F),
        ("CameraOffsetZ", F),
        ("CameraPosChaseRate", F),
        ("CameraFacingChaseRate", F),
        ("CameraEnteringZoom", F),
        ("CameraSeatZoomMin", F),
        ("CameraSeatZoomMax", F),
    ];
    schema("VehicleSeat", &names)
}

pub(crate) fn vehicle_ui_indicator_schema() -> Schema {
    schema(
        "VehicleUIIndicator",
        &[
            ("ID", FieldType::UInt32),
            ("BackgroundTexture", FieldType::String),
        ],
    )
}

pub(crate) fn override_spell_data_schema() -> Schema {
    use FieldType::UInt32 as U;
    let mut f: Vec<(&str, FieldType)> = vec![("ID", U)];
    f.extend(std::iter::repeat_n(("Spells", U), OVERRIDE_SPELLS));
    f.push(("Flags", U));
    schema("OverrideSpellData", &f)
}

pub(crate) fn vehicle_ui_ind_seat_schema() -> Schema {
    use FieldType::{Float32 as F, UInt32 as U};
    schema(
        "VehicleUIIndSeat",
        &[
            ("ID", U),
            ("VehicleUIIndicatorID", U),
            ("VirtualSeatIndex", U),
            ("XPos", F),
            ("YPos", F),
        ],
    )
}

fn f32s<const N: usize>(r: &benilla_dbc::Record, from: usize) -> Option<[f32; N]> {
    let mut out = [0.0; N];
    for (i, v) in out.iter_mut().enumerate() {
        *v = f32_at(r, from + i)?;
    }
    Some(out)
}

fn i32s<const N: usize>(r: &benilla_dbc::Record, from: usize) -> Option<[i32; N]> {
    let mut out = [0; N];
    for (i, v) in out.iter_mut().enumerate() {
        *v = i32_at(r, from + i)?;
    }
    Some(out)
}

fn u32s<const N: usize>(r: &benilla_dbc::Record, from: usize) -> Option<[u32; N]> {
    let mut out = [0; N];
    for (i, v) in out.iter_mut().enumerate() {
        *v = u32_at(r, from + i)?;
    }
    Some(out)
}

fn vehicle_row(r: &benilla_dbc::Record) -> Option<VehicleRec> {
    Some(VehicleRec {
        id: u32_at(r, 0)?,
        flags: u32_at(r, 1)?,
        turn_speed: f32_at(r, 2)?,
        pitch_speed: f32_at(r, 3)?,
        pitch_min: f32_at(r, 4)?,
        pitch_max: f32_at(r, 5)?,
        seat_ids: u32s(r, 6)?,
        mouse_look_offset_pitch: f32_at(r, 14)?,
        camera_fade_dist_scalar_min: f32_at(r, 15)?,
        camera_fade_dist_scalar_max: f32_at(r, 16)?,
        camera_pitch_offset: f32_at(r, 17)?,
        facing_limit_right: f32_at(r, 18)?,
        facing_limit_left: f32_at(r, 19)?,
        camera_yaw_offset: f32_at(r, 33)?,
        ui_locomotion_type: u32_at(r, 34)?,
        ui_indicator_id: u32_at(r, 36)?,
        power_display_ids: u32s(r, 37)?,
    })
}

fn seat_row(r: &benilla_dbc::Record) -> Option<VehicleSeatRec> {
    Some(VehicleSeatRec {
        id: u32_at(r, 0)?,
        flags: u32_at(r, 1)?,
        attachment_id: i32_at(r, 2)?,
        attachment_offset: f32s(r, 3)?,
        enter_pre_delay: f32_at(r, 6)?,
        enter_speed: f32_at(r, 7)?,
        enter_gravity: f32_at(r, 8)?,
        enter_min_duration: f32_at(r, 9)?,
        enter_max_duration: f32_at(r, 10)?,
        enter_min_arc_height: f32_at(r, 11)?,
        enter_max_arc_height: f32_at(r, 12)?,
        passenger_anims: i32s(r, 13)?,
        exit_pre_delay: f32_at(r, 19)?,
        exit_speed: f32_at(r, 20)?,
        exit_gravity: f32_at(r, 21)?,
        exit_min_duration: f32_at(r, 22)?,
        exit_max_duration: f32_at(r, 23)?,
        exit_min_arc_height: f32_at(r, 24)?,
        exit_max_arc_height: f32_at(r, 25)?,
        exit_anims: i32s(r, 26)?,
        passenger_yaw: f32_at(r, 29)?,
        passenger_pitch: f32_at(r, 30)?,
        passenger_roll: f32_at(r, 31)?,
        passenger_attachment_id: i32_at(r, 32)?,
        vehicle_anims: i32s(r, 33)?,
        vehicle_anim_bones: i32s(r, 36)?,
        vehicle_enter_anim_delay: f32_at(r, 39)?,
        vehicle_exit_anim_delay: f32_at(r, 40)?,
        vehicle_ability_display: i32_at(r, 41)?,
        enter_ui_sound_id: u32_at(r, 42)?,
        exit_ui_sound_id: u32_at(r, 43)?,
        ui_skin: i32_at(r, 44)?,
        flags_b: u32_at(r, 45)?,
        camera_entering_delay: f32_at(r, 46)?,
        camera_entering_duration: f32_at(r, 47)?,
        camera_exiting_delay: f32_at(r, 48)?,
        camera_exiting_duration: f32_at(r, 49)?,
        camera_offset: f32s(r, 50)?,
        camera_pos_chase_rate: f32_at(r, 53)?,
        camera_facing_chase_rate: f32_at(r, 54)?,
        camera_entering_zoom: f32_at(r, 55)?,
        camera_seat_zoom_min: f32_at(r, 56)?,
        camera_seat_zoom_max: f32_at(r, 57)?,
    })
}

/// Load the four vehicle tables from the patch chain.
pub fn load_vehicles(chain: &mut Chain) -> Result<VehicleCatalog> {
    let mut read = |path: &str, schema: Schema, what: &str| -> Result<benilla_dbc::RecordSet> {
        let bytes = chain
            .read_file(path)
            .with_context(|| format!("reading {path}"))?;
        parse(&bytes, schema, what)
    };
    let vehicles = read(VEHICLE, vehicle_schema(), "Vehicle")?
        .records()
        .iter()
        .filter_map(vehicle_row)
        .map(|v| (v.id, v))
        .collect();
    let seats = read(VEHICLE_SEAT, vehicle_seat_schema(), "VehicleSeat")?
        .records()
        .iter()
        .filter_map(seat_row)
        .map(|s| (s.id, s))
        .collect();
    let ind = read(
        VEHICLE_UI_INDICATOR,
        vehicle_ui_indicator_schema(),
        "VehicleUIIndicator",
    )?;
    let mut indicators: HashMap<u32, VehicleUiIndicator> = ind
        .records()
        .iter()
        .filter_map(|r| {
            Some((
                u32_at(r, 0)?,
                VehicleUiIndicator {
                    background_texture: str_at(&ind, r, 1).unwrap_or_default(),
                    seats: Vec::new(),
                },
            ))
        })
        .collect();
    // `0x614d90` scans the seat rows by id, from 1 up, for the indicator's id.
    let ind_seats = read(
        VEHICLE_UI_IND_SEAT,
        vehicle_ui_ind_seat_schema(),
        "VehicleUIIndSeat",
    )?;
    let mut rows: Vec<_> = ind_seats
        .records()
        .iter()
        .filter_map(|r| Some((u32_at(r, 0)?, r)))
        .collect();
    rows.sort_by_key(|&(id, _)| id);
    for (_, r) in rows {
        let (Some(owner), Some(index), Some(x), Some(y)) =
            (u32_at(r, 1), u32_at(r, 2), f32_at(r, 3), f32_at(r, 4))
        else {
            continue;
        };
        if let Some(ind) = indicators.get_mut(&owner) {
            ind.seats.push(IndicatorSeat {
                virtual_seat_index: index,
                x,
                y,
            });
        }
    }
    let override_spells = read(
        OVERRIDE_SPELL_DATA,
        override_spell_data_schema(),
        "OverrideSpellData",
    )?
    .records()
    .iter()
    .filter_map(|r| {
        let rec = OverrideSpellsRec {
            id: u32_at(r, 0)?,
            spells: std::array::from_fn(|i| u32_at(r, 1 + i).unwrap_or(0)),
            flags: u32_at(r, 1 + OVERRIDE_SPELLS).unwrap_or(0),
        };
        Some((rec.id, rec))
    })
    .collect();
    Ok(VehicleCatalog {
        vehicles,
        seats,
        indicators,
        override_spells,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The widths 12340's loaders check (`reference/3.3.5a-dbc.tsv`).
    #[test]
    fn the_schemas_hold_the_reference_widths() {
        for (schema, cols, bytes) in [
            (vehicle_schema(), 40, 160),
            (vehicle_seat_schema(), 58, 232),
            (vehicle_ui_indicator_schema(), 2, 8),
            (vehicle_ui_ind_seat_schema(), 5, 20),
            (override_spell_data_schema(), 12, 48),
        ] {
            assert_eq!(
                (schema.expanded_len(), schema.record_width()),
                (cols, bytes)
            );
        }
    }

    /// Every vehicle's seat slots name seats the seat table has, and every indicator a vehicle
    /// names exists.
    #[test]
    fn vehicles_name_real_seats_and_indicators() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_vehicles(&mut chain).expect("vehicle tables");
        assert!(cat.vehicles.len() > 100, "{} vehicles", cat.vehicles.len());
        for v in cat.vehicles.values() {
            for (slot, &id) in v.seat_ids.iter().enumerate() {
                if id != 0 {
                    assert!(
                        cat.vehicle_seat(v, slot).is_some(),
                        "vehicle {} seat {slot} names missing seat {id}",
                        v.id
                    );
                }
            }
            if v.ui_indicator_id != 0 {
                assert!(
                    cat.indicator(v.ui_indicator_id).is_some(),
                    "vehicle {} names missing indicator {}",
                    v.id,
                    v.ui_indicator_id
                );
            }
        }
    }
}
