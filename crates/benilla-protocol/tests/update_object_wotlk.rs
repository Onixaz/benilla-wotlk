//! 3.3.5a `SMSG_UPDATE_OBJECT` and `MSG_MOVE_*` wire tests: each body is written the way
//! AzerothCore writes it (`UpdateData::BuildPacket`, `Object::BuildMovementUpdate`,
//! `Unit::BuildMovementPacket`, `Object::BuildValuesUpdate`) and must decode into the events the
//! 1.12.1 path gives, with nothing left unread.

use benilla_protocol::events::{decode, EntityKind, SessionEvent};
use benilla_protocol::messages::{self, opcode, ServerPacket};
use benilla_protocol::wire::write_packed_guid;

const PLAYER: u64 = 0x0000_0000_0000_0007;
const CREATURE: u64 = 0xF130_0000_0001_0002;
const GAMEOBJECT: u64 = 0xF110_0000_0003_0003;
const MO_TRANSPORT: u64 = 0x1FC0_0000_0000_0005;

#[derive(Default)]
struct Body(Vec<u8>);

impl Body {
    fn u8(&mut self, v: u8) -> &mut Self {
        self.0.push(v);
        self
    }
    fn u16(&mut self, v: u16) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn u32(&mut self, v: u32) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn u64(&mut self, v: u64) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn f32(&mut self, v: f32) -> &mut Self {
        self.u32(v.to_bits())
    }
    fn xyz(&mut self, v: [f32; 3]) -> &mut Self {
        self.f32(v[0]).f32(v[1]).f32(v[2])
    }
    fn guid(&mut self, g: u64) -> &mut Self {
        write_packed_guid(g, &mut self.0).unwrap();
        self
    }
    /// `Object::BuildValuesUpdate`: block count, the mask, then each set field ascending.
    fn fields(&mut self, pairs: &[(u16, u32)]) -> &mut Self {
        let mut pairs = pairs.to_vec();
        pairs.sort_unstable();
        let words = usize::from(pairs.last().map_or(0, |p| p.0 / 32 + 1));
        let mut mask = vec![0u32; words];
        for (i, _) in &pairs {
            mask[usize::from(i / 32)] |= 1 << (i % 32);
        }
        self.u8(words as u8);
        for w in mask {
            self.u32(w);
        }
        for (_, v) in pairs {
            self.u32(v);
        }
        self
    }
    /// The nine `LIVING` speeds: walk, run, run back, swim, swim back, flight, flight back, turn
    /// rate, pitch rate.
    fn speeds(&mut self) -> &mut Self {
        for s in [2.5, 7.0, 4.5, 4.722222, 2.5, 7.0, 4.5, 3.141594, 3.141594] {
            self.f32(s);
        }
        self
    }
}

fn parse(op: u16, body: &[u8]) -> ServerPacket {
    let (packet, tail) = messages::parse_server_with_tail(op, body).unwrap();
    assert_eq!(tail, 0, "bytes left unread");
    packet
}

fn world_entry_update() -> Vec<u8> {
    let mut b = Body::default();
    b.u32(5);

    // Our own player: CREATE_OBJECT2, `SELF | LIVING | STATIONARY_POSITION`, standing.
    b.u8(3).guid(PLAYER).u8(4).u16(0x0061);
    b.u32(0).u16(0).u32(1_000);
    b.xyz([-8949.95, -132.493, 83.5312]).f32(0.5);
    b.u32(0); // fall time
    b.speeds();
    b.fields(&[
        (0, PLAYER as u32),
        (2, 0x19),
        (4, 1.0f32.to_bits()),
        (23, 0x0100_0101), // human warrior, male, rage
        (24, 60),
        (32, 60),
        (54, 1),
        (55, 1),
        (67, 49),
        (68, 49),
        (153, 0x0403_0201),
        (283 + 2 * 4, 6_096), // chest
    ]);

    // A creature on a ship, mid-fall: `LIVING | STATIONARY_POSITION`, `ONTRANSPORT | FALLING |
    // FORWARD`, `INTERPOLATED_MOVEMENT` adding a second transport time.
    b.u8(2).guid(CREATURE).u8(3).u16(0x0060);
    b.u32(0x200 | 0x1000 | 0x1).u16(0x400).u32(2_000);
    b.xyz([10.0, 20.0, 30.0]).f32(1.5);
    b.guid(MO_TRANSPORT).xyz([1.0, 2.0, 3.0]).f32(0.25);
    b.u32(77).u8(0xff).u32(88);
    b.u32(120); // fall time
    b.f32(-7.9).f32(0.0).f32(1.0).f32(7.0); // zspeed, sin, cos, xy speed
    b.speeds();
    b.fields(&[(2, 0x9), (24, 100), (67, 1_234), (82, 0x80)]);

    // A mailbox: `LOWGUID | STATIONARY_POSITION | POSITION | ROTATION`.
    b.u8(2).guid(GAMEOBJECT).u8(5).u16(0x0350);
    b.u8(0)
        .xyz([100.0, 200.0, 300.0])
        .xyz([100.0, 200.0, 300.0]);
    b.f32(1.0).f32(0.0);
    b.u32(3); // low guid
              // A quarter turn about z, packed (`GameObject::UpdatePackedRotation`): z = 0.7071068 ×
              // 2^20 in the low 21 bits.
    b.u64(741_455);
    b.fields(&[(2, 0x21), (8, 99), (17, 0xFF00_1301)]);

    // The creature's health drops.
    b.u8(0).guid(CREATURE).fields(&[(24, 50)]);

    b.u8(4).u32(1).guid(0x10);
    b.0
}

#[test]
fn a_3_3_5a_world_entry_update_decodes_into_the_shared_events() {
    let events = decode(parse(opcode::SMSG_UPDATE_OBJECT, &world_entry_update()));

    let create = |want: u64| {
        events
            .iter()
            .find(|e| matches!(e, SessionEvent::ObjectCreate { guid, .. } if *guid == want))
            .unwrap_or_else(|| panic!("no create for {want:#x}"))
    };

    let SessionEvent::ObjectCreate {
        kind,
        display_id,
        position,
        orientation,
        scale,
        speeds,
        mover,
        fields,
        ..
    } = create(PLAYER)
    else {
        unreachable!()
    };
    assert_eq!(*kind, EntityKind::Player);
    assert_eq!(*display_id, Some(49));
    assert_eq!(*position, [-8949.95, -132.493, 83.5312]);
    assert_eq!(*orientation, 0.5);
    assert_eq!(*scale, 1.0);
    let speeds = speeds.unwrap();
    assert_eq!(
        (speeds.walk, speeds.run, speeds.turn_rate),
        (2.5, 7.0, 3.141594)
    );
    assert_eq!(mover.unwrap().flags, 0);
    assert_eq!(fields.unit_health(), Some(60));
    assert_eq!(fields.unit_max_health(), Some(60));
    assert_eq!(fields.unit_level(), Some(1));
    assert_eq!(fields.unit_race(), Some(1));
    assert_eq!(fields.unit_power_type(), 1);
    assert_eq!(fields.player_skin(), Some(1));
    assert_eq!(fields.player_visible_item_entry(4), Some(6_096));
    // Absent in a create reads zero, as on 1.12.1.
    assert_eq!(fields.player_xp(), Some(0));

    let SessionEvent::ObjectCreate {
        kind,
        display_id,
        position,
        mover,
        transport,
        fields,
        ..
    } = create(CREATURE)
    else {
        unreachable!()
    };
    assert_eq!(*kind, EntityKind::Unit);
    assert_eq!(*display_id, Some(1_234));
    assert_eq!(*position, [10.0, 20.0, 30.0]);
    // 1.12.1's bits: forward, jumping 0x2000, on transport 0x0200_0000.
    assert_eq!(mover.unwrap().flags, 0x1 | 0x2000 | 0x0200_0000);
    let transport = transport.unwrap();
    assert_eq!(transport.guid, MO_TRANSPORT);
    assert_eq!(
        [transport.pos.x, transport.pos.y, transport.pos.z],
        [1.0, 2.0, 3.0]
    );
    assert_eq!(transport.orientation, 0.25);
    assert_eq!(fields.unit_npc_flags(), 0x80); // a vendor, in 3.3.5a bits
    assert_eq!(fields.unit_health(), Some(100));

    let SessionEvent::ObjectCreate {
        kind,
        display_id,
        position,
        orientation,
        speeds,
        fields,
        ..
    } = create(GAMEOBJECT)
    else {
        unreachable!()
    };
    assert_eq!(*kind, EntityKind::GameObject);
    assert_eq!(*display_id, Some(99));
    assert_eq!(*position, [100.0, 200.0, 300.0]);
    assert_eq!(*orientation, 1.0);
    assert!(speeds.is_none());
    assert_eq!(fields.gameobject_type_id(), 19);
    assert_eq!(fields.gameobject_state(), Some(1));
    let q = fields.gameobject_rotation().unwrap();
    assert!(
        (q[2] - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-5,
        "{q:?}"
    );
    assert!(
        (q[3] - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-5,
        "{q:?}"
    );

    let values = events
        .iter()
        .find_map(|e| match e {
            SessionEvent::ObjectValues { guid, fields } if *guid == CREATURE => Some(fields),
            _ => None,
        })
        .expect("values delta");
    assert_eq!(values.unit_health(), Some(50));
    // A delta leaves what it does not carry unknown.
    assert_eq!(values.unit_level(), None);

    assert!(events
        .iter()
        .any(|e| matches!(e, SessionEvent::ObjectsRemoved(g) if g == &[0x10])));
}

#[test]
fn the_compressed_3_3_5a_update_decodes_the_same() {
    use std::io::Write;
    let inner = world_entry_update();
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    z.write_all(&inner).unwrap();
    let mut body = (inner.len() as u32).to_le_bytes().to_vec();
    body.extend(z.finish().unwrap());
    let ServerPacket::UpdateObject { objects } =
        parse(opcode::SMSG_COMPRESSED_UPDATE_OBJECT, &body)
    else {
        panic!("not an update")
    };
    assert_eq!(objects.len(), 5);
}

#[test]
fn a_3_3_5a_move_relay_carries_its_pitch_and_1_12_1_flags() {
    // `WriteMovementInfo`: packed guid, then a swimming forward mover with its pitch.
    let mut b = Body::default();
    b.guid(PLAYER);
    b.u32(0x20_0000 | 0x1).u16(0).u32(3_000);
    b.xyz([1.0, 2.0, -3.0]).f32(0.75);
    b.f32(-0.3); // pitch
    b.u32(0);
    let ServerPacket::PlayerMove {
        guid,
        flags,
        position,
        pitch,
        transport,
        jump,
        ..
    } = parse(opcode::MSG_MOVE_HEARTBEAT, &b.0)
    else {
        panic!("not a relay")
    };
    assert_eq!(guid, PLAYER);
    assert_eq!(flags, 0x20_0000 | 0x1);
    assert_eq!([position.x, position.y, position.z], [1.0, 2.0, -3.0]);
    assert_eq!(pitch, -0.3);
    assert!(transport.is_none() && jump.is_none());
}

#[test]
fn a_3_3_5a_body_with_no_decoder_yet_is_refused_not_read_as_1_12_1() {
    assert!(messages::parse_server_with_tail(opcode::SMSG_FRIEND_LIST, &[0; 64]).is_err());
}

#[test]
fn destroy_and_time_speed_read_their_3_3_5a_tails() {
    let mut b = Body::default();
    b.u64(CREATURE).u8(1);
    assert!(matches!(
        parse(opcode::SMSG_DESTROY_OBJECT, &b.0),
        ServerPacket::DestroyObject { guid: CREATURE }
    ));
    let mut b = Body::default();
    b.u32(0).f32(0.016_666_67).u32(0);
    assert!(matches!(
        parse(opcode::SMSG_LOGIN_SETTIMESPEED, &b.0),
        ServerPacket::TimeSpeed { .. }
    ));
}
