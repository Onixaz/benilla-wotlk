use std::io::{self, Read};

use crate::wire::{capacity_hint, read_u32_le, read_u8, Vector3d};

use super::movement::ObjectType;

use index::*;
pub use index::{ARENA_TEAM_INFO_STRIDE, FIELD_PLAYER_SKILL_INFO_1_1, QUEST_LOG_SLOT_FIELDS};

/// The spell schools the cost runs hold.
const SPELL_SCHOOLS: u16 = 7;

/// `PLAYER_EXPLORED_ZONES_1`'s word count, 128: 4096 bits, Northrend's later zones at 2048 and up.
pub const PLAYER_EXPLORED_ZONES_SLOTS: u16 = 128;
/// The known-titles bitmask's width in bits.
pub const PLAYER_KNOWN_TITLE_BITS: u32 = 192;
/// The currency-token slots, player slots 118..149.
pub const CURRENCY_TOKEN_SLOTS: u8 = 32;
/// The glyph sockets, six in 3.3.5a (`0x5b71e0`).
pub const PLAYER_GLYPH_SOCKETS: u8 = 6;
/// The combat ratings, 25 in 3.3.5a (the rating read refuses an index past `0x19`).
pub const PLAYER_COMBAT_RATINGS: u8 = 25;

/// The 3.3.5a descriptor indices, for a field watch or a fixture: the same constants the accessors
/// read, so the two agree.
pub mod field {
    pub use super::index::*;
    /// The item block's local writes: the read check's translated bit, a socket result's enchants.
    pub use super::player::item_enchant_field;
    /// The player slot a slot-array guid word holds.
    pub use super::player::player_slot_of_field;
    /// The bit a watcher tests on a raw `UNIT_DYNAMIC_FLAGS` edge (feign death).
    pub use super::unit::UNIT_DYNFLAG_DEAD;
    /// `GAMEOBJECT_BYTES_1` byte 0, the state, out of a raw word or edge value.
    pub fn gameobject_state_of(bytes_1: u32) -> u32 {
        bytes_1 & 0xff
    }
    /// `GAMEOBJECT_DYNAMIC`'s low half, the dynamic flags, out of a raw word or edge value.
    pub fn gameobject_dyn_flags_of(dynamic: u32) -> u32 {
        dynamic & 0xffff
    }
}

/// The quest log's slots, 25 in 3.3.5a: 12340 walks them `< 0x1a` at stride `0x14` (`0x6d8086`).
pub const PLAYER_QUEST_LOG_SLOTS: u8 = 25;

/// One `PLAYER_QUEST_LOG` slot, the durable quest state; `SMSG_QUESTUPDATE_*` are only toasts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuestLogSlot {
    /// 0 for an empty slot.
    pub quest_id: u32,
    /// Four kill, cast and interact counters, `u16` each over two fields (12340 reads counter `i`
    /// as `movzx WORD [slot + 8 + 2i]`, `0x5e161c`); the client counts item objectives itself.
    pub counters: [u16; 4],
    /// The state field's low byte: a [`quest_slot_state`] bit, 0 in progress (`0x6d80a9` tests
    /// `& 2` for a failed quest).
    pub state: u8,
    /// The absolute end time of a timed quest, else 0.
    pub timer: u32,
}

/// `QUEST_STATE_*` bits of [`QuestLogSlot::state`].
pub mod quest_slot_state {
    pub const COMPLETE: u8 = 0x01;
    pub const FAIL: u8 = 0x02;
}
/// `PLAYER_SKILL_INFO` slots on the wire, 3 fields each.
pub const PLAYER_SKILL_SLOTS: u8 = 128;

/// One `PLAYER_SKILL_INFO` slot, three dwords (`Player.cpp:90-99`): `id | step << 16`,
/// `value | max << 16`, `temp_bonus | perm_bonus << 16` with signed bonuses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlayerSkillSlot {
    /// A `SkillLine.dbc` id, 0 for an unused slot.
    pub skill_id: u16,
    /// The tier step, 0 unless a tier-raising effect is live.
    pub step: u16,
    pub value: u16,
    pub max: u16,
    /// From auras, consumables and enchants; a malus is negative.
    pub temp_bonus: i16,
    /// From talents.
    pub perm_bonus: i16,
}

/// A corpse's own appearance, snapshotted at death: the seven `CORPSE_FIELD_BYTES_1`/`_2` bytes the
/// client's dress loads, not the owner's live `PLAYER_BYTES`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CorpseLook {
    pub race: u8,
    pub sex: u8,
    pub skin: u8,
    pub face: u8,
    pub hair_style: u8,
    pub hair_color: u8,
    pub facial_hair: u8,
}

/// A sparse update-field set: the wire's block mask and a dense value array. A CREATE carries only
/// nonzero fields (`Object::_SetCreateBits`) into the client's zeroed buffer, so an absent field
/// within the created type's descriptor reads `Some(0)`; in a `Values` delta, or past the
/// descriptor's end, it reads `None`.
#[derive(Debug, Clone, Default)]
pub struct ObjectFields {
    /// The wire's block mask, verbatim (`index = word * 32 + bit`).
    present: Vec<u32>,
    /// Always `present.len() * 32` long; an unset slot holds 0 but must be read through the mask.
    values: Vec<u32>,
    /// The created type's [`descriptor_len`], below which absent reads 0; 0 for a delta or fixture.
    descriptor_end: u16,
    /// The client's own copies read in place of a field, `None` while none stands.
    shadow: Option<Box<Shadow>>,
    /// A create's values the movement block carries, not the descriptor; `None` on a delta.
    placed: Option<Box<Placed>>,
}

/// What a 3.3.5a create carries in its movement block for which no descriptor field exists: a
/// GameObject's own rotation (`UPDATEFLAG_ROTATION`) and a dynamic object's anchor
/// (`UPDATEFLAG_STATIONARY_POSITION`).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Placed {
    /// `(x, y, z, w)`.
    rotation: Option<[f32; 4]>,
    /// Raw WoW coordinates and facing.
    position: Option<([f32; 3], f32)>,
}

/// What the client keeps beside a unit's descriptor and reads in its place: the power
/// `SMSG_POWER_UPDATE` sets (the predicted-power set `0x722c50`, `[unit+0xfb4]`), the health
/// `SMSG_HEALTH_UPDATE` sets (`[unit+0xfb0]`, `0x716d66`) and the mount `SMSG_DISMOUNT` drops ahead
/// of the field (`0x741a40`). A move of the shadowed field drops the
/// copy, as its field handler re-seeds it (`0x7234d0`, `0x7235c0`, `0x723620`). benilla predicts no
/// spend, so the handler's keep-a-lower-prediction leg for our own unit never differs.
#[derive(Debug, Clone, Default)]
struct Shadow {
    /// By power type, 0..7.
    power: [Option<u32>; 7],
    health: Option<u32>,
    dismounted: bool,
    /// The mount a dismount kept while passengers ride (`[unit+0xa30]` bit `0x10000000` set, its
    /// display left in `[unit+0x9c0]`), 0 when the bit is clear.
    mount_kept: u32,
    /// The form our own cancel dropped ahead of the server (`[unit+0x9f4]`, `0x721d82`), until the form byte next moves (`0x6da174`).
    form_suppressed: bool,
}

/// An object's descriptor length in dwords: the `*_END` of its innermost block, so a Player spans
/// OBJECT, UNIT and PLAYER.
fn descriptor_len(object_type: ObjectType) -> u16 {
    match object_type {
        ObjectType::Object => OBJECT_END,
        ObjectType::Item => ITEM_END,
        ObjectType::Container => CONTAINER_END,
        ObjectType::Unit => UNIT_END,
        ObjectType::Player => PLAYER_END,
        ObjectType::GameObject => GAMEOBJECT_END,
        ObjectType::DynamicObject => DYNAMICOBJECT_END,
        ObjectType::Corpse => CORPSE_END,
    }
}

/// Mask words of the widest descriptor, `PLAYER_END`.
const MAX_MASK_WORDS: usize = (PLAYER_END as usize).div_ceil(32);

impl ObjectFields {
    pub(super) fn read(r: &mut impl Read) -> io::Result<Self> {
        let amount_of_blocks = read_u8(r)?;
        let mut present = Vec::with_capacity(capacity_hint(amount_of_blocks, MAX_MASK_WORDS));
        for _ in 0..amount_of_blocks {
            present.push(read_u32_le(r)?);
        }
        let mut values = vec![0u32; present.len() * 32];
        for (word, block) in present.iter().enumerate() {
            for bit in 0..32 {
                if block & (1u32 << bit) != 0 {
                    values[word * 32 + bit] = read_u32_le(r)?;
                }
            }
        }
        Ok(Self {
            present,
            values,
            descriptor_end: 0,
            shadow: None,
            placed: None,
        })
    }

    /// Keep what a create's movement block carried that no field holds: a GameObject's rotation
    /// and a dynamic object's position and facing.
    pub(in crate::messages::update_object) fn place_movement_values(
        &mut self,
        object_type: ObjectType,
        position: Option<(Vector3d, f32)>,
        rotation: Option<[f32; 4]>,
    ) {
        let placed = match (object_type, position, rotation) {
            (ObjectType::GameObject, _, Some(q)) => Placed {
                rotation: Some(q),
                position: None,
            },
            (ObjectType::DynamicObject, Some((p, o)), _) => Placed {
                rotation: None,
                position: Some(([p.x, p.y, p.z], o)),
            },
            _ => return,
        };
        self.placed = Some(Box::new(placed));
    }

    /// Marks a CREATE snapshot of `object_type`, so absent fields in its descriptor read 0.
    pub fn into_created(mut self, object_type: ObjectType) -> Self {
        self.descriptor_end = descriptor_len(object_type);
        self
    }

    /// A fixture from `(index, value)` pairs; [`Self::into_created`] makes it a create.
    pub fn from_pairs(pairs: &[(u16, u32)]) -> Self {
        let mut this = Self::default();
        for &(index, value) in pairs {
            this.insert(index, value);
        }
        this
    }

    /// `CORPSE_FIELD_OWNER`: the dead player, only on a corpse.
    pub fn corpse_owner(&self) -> Option<u64> {
        self.get_guid(FIELD_CORPSE_OWNER).filter(|&g| g != 0)
    }

    /// The body's `CreatureDisplayInfo` id, the owner's native display (`Player.cpp:4809`). A bone
    /// pile keeps it, but the reference ignores it there and builds the skeleton from race and sex.
    pub fn corpse_display_id(&self) -> Option<u32> {
        self.get_u32(FIELD_CORPSE_DISPLAY_ID).filter(|&d| d != 0)
    }

    /// The piece in equipment slot `slot` (0..18) as `(ItemDisplayInfo id, InventoryType)`, packed
    /// `display | type << 24` (`Player.cpp:4822`); unlike `PLAYER_VISIBLE_ITEM`, not an item entry.
    pub fn corpse_item(&self, slot: u8) -> Option<(u32, u8)> {
        let raw = self.get_u32(FIELD_CORPSE_ITEM + u16::from(slot))?;
        let display = raw & 0x00ff_ffff;
        (display != 0).then_some((display, (raw >> 24) as u8))
    }

    /// The owner's guild id at death, 0 for none; the client builds the corpse's tabard crest from
    /// it.
    pub fn corpse_guild(&self) -> u32 {
        self.get_u32(FIELD_CORPSE_GUILD).unwrap_or(0)
    }

    /// Race, gender and skin in bytes 1..3; byte 0 is unused (`Corpse.cpp:228`).
    fn corpse_bytes_1(&self) -> Option<u32> {
        self.get_u32(FIELD_CORPSE_BYTES_1)
    }
    /// Face, hair style, hair colour and facial hair (`Corpse.cpp:229`).
    fn corpse_bytes_2(&self) -> Option<u32> {
        self.get_u32(FIELD_CORPSE_BYTES_2)
    }
    /// The dead player's race, `CORPSE_FIELD_BYTES_1` byte 1, the one corpse field the reaction
    /// gate reads; 0, which no `ChrRaces` row names, when absent.
    pub fn corpse_race(&self) -> u8 {
        self.corpse_bytes_1().map_or(0, |b1| (b1 >> 8) as u8)
    }
    /// The corpse's appearance, `None` in a delta that lacks either BYTES word.
    pub fn corpse_look(&self) -> Option<CorpseLook> {
        let (b1, b2) = (self.corpse_bytes_1()?, self.corpse_bytes_2()?);
        Some(CorpseLook {
            race: (b1 >> 8) as u8,
            sex: (b1 >> 16) as u8,
            skin: (b1 >> 24) as u8,
            face: b2 as u8,
            hair_style: (b2 >> 8) as u8,
            hair_color: (b2 >> 16) as u8,
            facial_hair: (b2 >> 24) as u8,
        })
    }

    /// `CORPSE_FIELD_FLAGS`, 0 when absent.
    pub fn corpse_flags(&self) -> u32 {
        self.get_u32(FIELD_CORPSE_FLAGS).unwrap_or(0)
    }
    /// [`Self::corpse_flags`] keeping absence: untouched flags in a delta are `None`, not all
    /// clear. A reader acting on a change must use this one.
    pub fn corpse_flags_present(&self) -> Option<u32> {
        self.get_u32(FIELD_CORPSE_FLAGS)
    }
    /// `CORPSE_FLAG_BONES`: a bone pile, which the reference tests first; it wears nothing and
    /// takes its model from race and sex.
    pub fn corpse_is_bones(&self) -> bool {
        self.corpse_flags() & 0x01 != 0
    }
    /// `CORPSE_FLAG_HIDE_HELM`, the owner's setting at death; the client skips slot 0.
    pub fn corpse_hides_helm(&self) -> bool {
        self.corpse_flags() & 0x08 != 0
    }
    /// `CORPSE_FLAG_HIDE_CLOAK`: the same for slot 14.
    pub fn corpse_hides_cloak(&self) -> bool {
        self.corpse_flags() & 0x10 != 0
    }

    /// `CORPSE_DYNFLAG_LOOTABLE` (`Map.cpp:3655`): the bone pile has insignia; the reference gates
    /// the loot highlight and the `CMSG_LOOT` click on it.
    pub fn corpse_lootable(&self) -> bool {
        self.get_u32(FIELD_CORPSE_DYNAMIC_FLAGS).unwrap_or(0) & 0x01 != 0
    }

    /// `CORPSE_FLAG_LOOTABLE`: the battleground insignia is takeable, by spell 22027 "Remove
    /// Insignia". The cursor and right-click test it; the corpse's PvP flag is bit 2 (`UnitIsPVP`).
    pub fn corpse_pvp_insignia(&self) -> bool {
        self.corpse_flags() & 0x20 != 0
    }

    /// `DYNAMICOBJECT_CASTER`, the ground caster, at the same index as a corpse's owner.
    pub fn dynamicobject_caster(&self) -> Option<u64> {
        self.get_guid(FIELD_DYNAMICOBJECT_CASTER)
            .filter(|&g| g != 0)
    }
    /// `DYNAMICOBJECT_BYTES`, the type byte: 1 (area spell) for a persistent-area cast.
    pub fn dynamicobject_bytes(&self) -> Option<u32> {
        self.get_u32(FIELD_DYNAMICOBJECT_BYTES)
    }
    /// The anchoring spell, the root of the ground-targeted visual chain.
    pub fn dynamicobject_spell_id(&self) -> Option<u32> {
        self.get_u32(FIELD_DYNAMICOBJECT_SPELLID)
            .filter(|&s| s != 0)
    }
    /// The area's radius in yards, which the server resolves from `SpellRadius.dbc`.
    pub fn dynamicobject_radius(&self) -> Option<f32> {
        self.get_f32(FIELD_DYNAMICOBJECT_RADIUS)
    }
    /// `DYNAMICOBJECT_CASTTIME`, the server clock of the cast that made it (`0x804cc0`).
    pub fn dynamicobject_cast_time(&self) -> Option<u32> {
        self.get_u32(FIELD_DYNAMICOBJECT_CASTTIME)
    }
    /// The anchored point and facing in raw WoW coordinates: the create's movement-block position,
    /// as no descriptor field holds it.
    pub fn dynamicobject_position(&self) -> Option<([f32; 3], f32)> {
        self.placed.as_deref()?.position
    }
    /// A GameObject's own rotation `(x, y, z, w)`, from its create's movement block
    /// (`UPDATEFLAG_ROTATION`); the reference places a GameObject by it.
    pub fn gameobject_rotation(&self) -> Option<[f32; 4]> {
        self.placed.as_deref()?.rotation
    }

    /// Whether the mask carries the field, ignoring the created store's absent-is-zero.
    fn contains(&self, index: u16) -> bool {
        self.present
            .get(usize::from(index / 32))
            .is_some_and(|w| w & (1u32 << (index % 32)) != 0)
    }

    fn get_raw(&self, index: u16) -> Option<u32> {
        self.contains(index)
            .then(|| self.values[usize::from(index)])
    }

    fn insert(&mut self, index: u16, value: u32) {
        // A `u16` index bounds the store at 64 Ki values, whatever the wire says.
        let word = usize::from(index / 32);
        if word >= self.present.len() {
            self.present.resize(word + 1, 0);
            self.values.resize(self.present.len() * 32, 0);
        }
        self.present[word] |= 1u32 << (index % 32);
        self.values[usize::from(index)] = value;
    }

    fn get_guid(&self, index: u16) -> Option<u64> {
        // Raw on purpose: a guid is present iff its low half is, even on a created store.
        let lo = self.get_raw(index)?;
        let hi = self.get_u32(index + 1).unwrap_or(0);
        Some(u64::from(lo) | (u64::from(hi) << 32))
    }

    fn get_u32(&self, index: u16) -> Option<u32> {
        self.get_raw(index)
            .or((index < self.descriptor_end).then_some(0))
    }
    fn get_f32(&self, index: u16) -> Option<f32> {
        self.get_u32(index).map(f32::from_bits)
    }
    fn get_i32(&self, index: u16) -> Option<i32> {
        self.get_u32(index).map(|v| v as i32)
    }
    /// A `TWO_SHORT` field as its (low, high) halves.
    fn get_u16_pair(&self, index: u16) -> Option<(u16, u16)> {
        self.get_u32(index).map(|v| (v as u16, (v >> 16) as u16))
    }

    /// Folds in an update: a `Values` delta overlays, since a field going to 0 is sent as 0; a
    /// re-CREATE replaces, as overlaying it would keep fields that have since dropped to 0.
    pub fn merge(&mut self, delta: ObjectFields) {
        self.merge_diff(delta, |_, _, _| {});
    }

    /// [`Self::merge`], calling `changed(index, old, new)` once per changed dword in ascending
    /// order, as the client's `CMirrorHandler` pass does: a resend of the same value reports
    /// nothing, and an absent old value reads 0. A first create is seeded, never merged.
    pub fn merge_diff(&mut self, delta: ObjectFields, mut changed: impl FnMut(u16, u32, u32)) {
        if delta.descriptor_end != 0 {
            let words = self.present.len().max(delta.present.len());
            for word in 0..words {
                let mask = self.present.get(word).copied().unwrap_or(0)
                    | delta.present.get(word).copied().unwrap_or(0);
                if mask == 0 {
                    continue;
                }
                for bit in 0..32u16 {
                    if mask & (1u32 << bit) == 0 {
                        continue;
                    }
                    let index = word as u16 * 32 + bit;
                    let (old, new) = (
                        self.get_raw(index).unwrap_or(0),
                        delta.get_raw(index).unwrap_or(0),
                    );
                    if old != new {
                        changed(index, old, new);
                        follow_shadow(&mut self.shadow, index, old, new);
                    }
                }
            }
            let shadow = self.shadow.take();
            *self = delta;
            self.shadow = shadow;
        } else {
            for (index, value) in delta.raw_fields() {
                let old = self.get_raw(index).unwrap_or(0);
                if old != value {
                    changed(index, old, value);
                    follow_shadow(&mut self.shadow, index, old, value);
                }
                self.insert(index, value);
            }
        }
    }

    /// `SMSG_POWER_UPDATE` (the predicted-power set `0x722c50`): power type `ty` reads `value`,
    /// capped at its maximum, until its field next moves. Whether the read moved; a type past the
    /// seven has no slot.
    pub fn set_predicted_power(&mut self, ty: u8, value: u32) -> bool {
        if usize::from(ty) >= 7 {
            return false;
        }
        let value = value.min(self.unit_max_power(ty).unwrap_or(0));
        if self.unit_power(ty) == Some(value) {
            return false;
        }
        self.shadow.get_or_insert_default().power[usize::from(ty)] = Some(value);
        true
    }

    /// `SMSG_HEALTH_UPDATE` (`0x716d20`): `UnitHealth` reads `value`, uncapped, until the health
    /// field next moves and its watcher re-seeds the copy (`0x73f35c`). Whether the read moved.
    pub fn set_predicted_health(&mut self, value: u32) -> bool {
        if self.unit_shown_health() == Some(value) {
            return false;
        }
        self.shadow.get_or_insert_default().health = Some(value);
        true
    }

    /// The predicted health while one stands.
    fn predicted_health(&self) -> Option<u32> {
        self.shadow.as_deref()?.health
    }

    /// `SMSG_DISMOUNT` (`0x741a40`): the mount reads 0 until its field next moves. Whether it
    /// dropped one; an unmounted unit is left alone.
    pub fn drop_mount(&mut self) -> bool {
        if self.unit_mount_display_id() == 0 {
            return false;
        }
        self.shadow.get_or_insert_default().dismounted = true;
        true
    }

    /// A dismount held while passengers ride (`0x741a7f`, `0x74046f`): `mount`, the unit's own
    /// copy, stays and the unit reads unmounted until the field moves to another display.
    pub fn hold_mount(&mut self, mount: u32) {
        if mount != 0 {
            self.shadow.get_or_insert_default().mount_kept = mount;
        }
    }

    /// The unit's own mount copy (`[unit+0x9c0]`): a held mount, else the field less a dropped one.
    pub fn unit_mount_copy(&self) -> u32 {
        match self.shadow.as_deref().map_or(0, |s| s.mount_kept) {
            0 => self.unit_mount_display_id(),
            kept => kept,
        }
    }

    /// Mounted (`0x51a230`): a mount copy and no held dismount.
    pub fn unit_is_mounted(&self) -> bool {
        self.shadow.as_deref().is_none_or(|s| s.mount_kept == 0) && self.unit_mount_display_id() > 0
    }

    /// The predicted power of type `ty` while one stands.
    fn predicted_power(&self, ty: u8) -> Option<u32> {
        self.shadow
            .as_deref()?
            .power
            .get(usize::from(ty))
            .copied()?
    }

    /// The early form cancel (`0x721cf0`): the form reads none until the form byte next moves.
    pub fn suppress_form(&mut self) {
        self.shadow.get_or_insert_default().form_suppressed = true;
    }

    /// Whether an early form cancel stands (`[unit+0x9f4]`).
    pub fn unit_form_suppressed(&self) -> bool {
        self.shadow.as_deref().is_some_and(|s| s.form_suppressed)
    }

    /// Whether `SMSG_DISMOUNT` dropped the mount the field still names.
    fn mount_dropped(&self) -> bool {
        self.shadow.as_deref().is_some_and(|s| s.dismounted)
    }
}

/// Drop the copy a moved field re-seeds: its power type's for a power or maximum, the new display
/// type's for `UNIT_FIELD_BYTES_0` (`0x723620`), the dropped mount for the mount field.
fn follow_shadow(shadow: &mut Option<Box<Shadow>>, index: u16, old: u32, new: u32) {
    let Some(s) = shadow.as_deref_mut() else {
        return;
    };
    let run = |base: u16| (base..base + 7).contains(&index).then(|| index - base);
    let slot = run(FIELD_UNIT_POWER1)
        .or_else(|| run(FIELD_UNIT_MAXPOWER1))
        .or_else(|| (index == FIELD_UNIT_BYTES_0).then_some((new >> 24) as u16));
    if let Some(slot) = slot.filter(|&t| t < 7) {
        s.power[usize::from(slot)] = None;
    }
    if index == FIELD_UNIT_MOUNTDISPLAYID {
        s.dismounted = false;
        // The change handler is a no-op on the copy it holds, and clears the hold on any other (`0x740450`).
        if new != s.mount_kept {
            s.mount_kept = 0;
        }
    }
    if index == FIELD_UNIT_HEALTH {
        s.health = None;
    }
    // The form byte's mirror handler (`0x6e538a`, offset `0x1d3`) clears the early cancel.
    if index == FIELD_UNIT_BYTES_2 && (old >> 24) != (new >> 24) {
        s.form_suppressed = false;
    }
}

impl ObjectFields {
    /// The type this was created as, from its descriptor length (the eight lengths are distinct).
    /// Unlike [`Self::object_type`] it tells a corpse from a unit, as index 34 needs.
    pub fn created_as(&self) -> Option<ObjectType> {
        const ALL: [ObjectType; 8] = [
            ObjectType::Object,
            ObjectType::Item,
            ObjectType::Container,
            ObjectType::Unit,
            ObjectType::Player,
            ObjectType::GameObject,
            ObjectType::DynamicObject,
            ObjectType::Corpse,
        ];
        (self.descriptor_end != 0)
            .then(|| {
                ALL.into_iter()
                    .find(|&t| descriptor_len(t) == self.descriptor_end)
            })
            .flatten()
    }

    /// No fields carried; the codec skips an empty `Values` delta.
    pub fn is_empty(&self) -> bool {
        self.present.iter().all(|&w| w == 0)
    }
    /// Every carried `(index, value)` pair in ascending order, for debugging and probes.
    pub fn raw_fields(&self) -> impl Iterator<Item = (u16, u32)> + '_ {
        self.present.iter().enumerate().flat_map(move |(word, &w)| {
            (0..32u16)
                .filter(move |bit| w & (1u32 << bit) != 0)
                .map(move |bit| {
                    let index = word as u16 * 32 + bit;
                    (index, self.values[usize::from(index)])
                })
        })
    }
}

mod index;
mod player;
mod unit;

pub use unit::{power_display_scale, OwnerFallback};

#[cfg(test)]
mod tests;
