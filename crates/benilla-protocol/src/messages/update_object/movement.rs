use crate::messages::movement::TransportPose;
use crate::wire::Vector3d;

/// Object class (the `TypeId` on a create packet).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectType {
    Object,
    Item,
    Container,
    Unit,
    Player,
    GameObject,
    DynamicObject,
    Corpse,
}

impl ObjectType {
    pub(super) fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Item,
            2 => Self::Container,
            3 => Self::Unit,
            4 => Self::Player,
            5 => Self::GameObject,
            6 => Self::DynamicObject,
            7 => Self::Corpse,
            _ => Self::Object,
        }
    }
}

/// The spline a `LIVING` unit is already riding when it streams in (`packet_builder.cpp:152`).
/// vmangos builds every spline as Catmull-Rom (`spline.cpp:52`), so the wire array is
/// `[phantom, p₀, …, pₙ, tail]` and [`Self::path`] is its walkable range `1 ..= len − 2`.
#[derive(Debug, Clone)]
pub struct CreateSpline {
    /// The travel-order polyline in raw WoW coords, start to endpoint.
    pub path: Vec<[f32; 3]>,
    /// The server's spline id (`MoveSpline::GetId`), as on `MonsterMove`.
    pub id: u32,
    /// Ms of the path already ridden (`MoveSpline::timePassed`); start the ride at now minus this.
    pub time_passed_ms: u32,
    /// Duration in ms at one constant speed, time uniform in arc length (`MoveSpline.cpp:125-135`).
    pub duration_ms: u32,
    /// `MoveSplineFlag::Flying`: a 3-D flight path (keep the spline's Z); clear for a ground walk.
    pub flying: bool,
    /// `MoveSplineFlag::Cyclic`: the server loops this path forever.
    pub cyclic: bool,
    /// `MoveSplineFlag::Runmode`; its absence forces walk mode on.
    pub run_mode: bool,
}

/// A `LIVING` block's live mover state: the `MOVEMENTFLAGS` word and the swim pitch that rides it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MoverState {
    /// The raw `MOVEMENTFLAGS` word (the movement block's `+0x40`).
    pub flags: u32,
    /// Swim pitch in radians, +up; on the wire only with `MOVEFLAG_SWIMMING`, else `0.0`.
    pub pitch: f32,
}

/// The decoded fields of a movement block; the rest is read and discarded to stay aligned.
pub struct MovementBlock {
    pub position: Option<(Vector3d, f32)>,
    /// A `LIVING` block's mover state. The reference merges these flags under the relay's
    /// `0x75a07dff` mask and commits the pitch at once, so a unit streaming in mid-swim is pitched
    /// on its first frame.
    pub mover: Option<MoverState>,
    /// A `LIVING` block's nine speeds in wire order: walk, run, run back, swim, swim back, flight,
    /// flight back, turn rate, pitch rate.
    pub speeds: Option<[f32; 9]>,
    /// A ship's or elevator's path progress in ms at create, the anchor its cycle runs from
    /// (`Object.cpp:590-605`); no other object sets `UPDATE_FLAG_TRANSPORT`.
    pub transport_progress: Option<u32>,
    /// A `LIVING` block's rider pose (`MOVEFLAG_ON_TRANSPORT`), local to the transport's frame.
    pub transport: Option<TransportPose>,
    /// The path this unit is already walking at create (`MOVEFLAG_SPLINE_ENABLED`).
    pub spline: Option<CreateSpline>,
}

impl MovementBlock {}
