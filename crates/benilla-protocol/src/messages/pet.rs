//! Pet messages: the pet action bar's state packet and the client verbs that drive it. The bar's
//! contents are the server's; its state (lit command, lit reaction, autocast) is the client's,
//! applied on the press, because the server never answers these verbs.

use std::io::{self, Read};

use crate::wire::{
    capacity_hint, read_cstring, read_u16_le, read_u32_le, read_u64_le, read_u8, Vector3d,
};

/// Pet bar slots, the ten words 12340's handler loops over (`0x5d6c77`).
pub const PET_ACTION_SLOTS: usize = 10;

/// The server's top byte for a passive spell: type 1, no autocast bits.
pub const PET_ACT_PASSIVE: u8 = 0x01;
/// A castable spell with autocast off: type 1 with [`PET_AUTOCAST_ALLOWED`].
pub const PET_ACT_DISABLED: u8 = 0x81;
/// A castable spell with autocast on: type 1 with both autocast bits.
pub const PET_ACT_ENABLED: u8 = 0xC1;
/// Type 7, an order: the action is a [`PET_COMMAND_STAY`]-family value (`0x5d4ec0`).
pub const PET_ACT_COMMAND: u8 = 0x07;
/// Type 6, a mode: the action is a [`PET_REACT_PASSIVE`]-family value (`0x5d4ec0`).
pub const PET_ACT_REACTION: u8 = 0x06;

/// The spell slot type; the client's type is the top byte `& 0x3F` (`0x5d4f60`).
pub const PET_TYPE_SPELL: u8 = 1;
/// The other types 12340's switch routes to the spell branch (`0x5d4ec0`, `0x5a80e0`).
pub const PET_TYPE_SPELL_EXTRA: std::ops::RangeInclusive<u8> = 8..=17;

/// Bit 31 of a packed word: the slot may autocast (`0x5d505f`), given a `Spell.dbc` record.
pub const PET_AUTOCAST_ALLOWED: u32 = 0x8000_0000;
/// Bit 30: autocast is running (`0x5d5074`); `TogglePetAutocast` flips it before the send.
pub const PET_AUTOCAST_ON: u32 = 0x4000_0000;

/// An order slot's action, indexing the `WAIT`/`FOLLOW`/`ATTACK`/`DISMISS` tokens (`0xa1a8c0`).
pub const PET_COMMAND_STAY: u32 = 0;
pub const PET_COMMAND_FOLLOW: u32 = 1;
pub const PET_COMMAND_ATTACK: u32 = 2;
pub const PET_COMMAND_DISMISS: u32 = 3;

/// A mode slot's action, indexing the `PASSIVE`/`DEFENSIVE`/`AGGRESSIVE` tokens (`0xa1a860`).
pub const PET_REACT_PASSIVE: u32 = 0;
pub const PET_REACT_DEFENSIVE: u32 = 1;
pub const PET_REACT_AGGRESSIVE: u32 = 2;

/// Bit 27 of [`PetSpells::state`], a vehicle's bar: the mode reads Passive (`0x5a7980`) and the
/// bar works only while we ride the pet (`0x5d3f77`).
pub const PET_STATE_VEHICLE: u32 = 0x0800_0000;

/// The pet's `UNIT_FIELD_FLAGS` bits that make it uncontrollable: stunned, fleeing, confused
/// (`CGUnit_C::IsControllable`, `0x5d3310`).
pub const PET_UNUSABLE_UNIT_FLAGS: u32 = 0x0004_0000 | 0x0040_0000 | 0x0080_0000;

/// The flag bit of a pet cooldown's category duration, which 12340 splits off before using the
/// rest as a duration (`0x5d6d19`, `0x5d6d2f`).
pub const PET_COOLDOWN_PERMANENT: u32 = 0x8000_0000;

/// One packed pet action word, as 12340 decodes it (`0x5d4ec0`): the action in bits 0-23
/// (`0x5d6c65`), the type in 24-29, autocast allowed in 31 and on in 30. The raw word is kept
/// because the client echoes it verbatim in [`pet_action`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PetActionEntry {
    pub packed: u32,
}

impl PetActionEntry {
    /// Bits 0-23: the spell id, command state or react state.
    pub fn action(self) -> u32 {
        self.packed & 0x00FF_FFFF
    }
    /// The client's slot type, bits 24-29.
    pub fn kind(self) -> u8 {
        ((self.packed >> 24) & 0x3F) as u8
    }
    /// Types 1 and 8-17 take the spell branch (`0x5d4ec0`). Deviation: any other type but 6 and
    /// 7 reads as nothing rather than the switch's fallthrough, which under-pushes its returns.
    pub fn is_spell(self) -> bool {
        let kind = self.kind();
        kind == PET_TYPE_SPELL || PET_TYPE_SPELL_EXTRA.contains(&kind)
    }
    /// Empty is a zero word (`0x5d4ec0`). A server's unused slot, `0x8100_0000`, takes the spell
    /// branch instead and comes back nil, since spell 0 has no `Spell.dbc` record.
    pub fn is_empty(self) -> bool {
        self.packed == 0
    }
    /// Bit 31; the client also requires a `Spell.dbc` record, which the catalog holder checks.
    pub fn autocast_allowed(self) -> bool {
        self.packed & PET_AUTOCAST_ALLOWED != 0
    }
    /// Bit 30: autocast is running.
    pub fn autocast_on(self) -> bool {
        self.packed & PET_AUTOCAST_ON != 0
    }
    /// Bit 30 set or cleared: `TogglePetAutocast`'s in-place flip (`0x5d5d20`), before the send.
    pub fn with_autocast(self, on: bool) -> Self {
        Self {
            packed: if on {
                self.packed | PET_AUTOCAST_ON
            } else {
                self.packed & !PET_AUTOCAST_ON
            },
        }
    }
}

impl From<u32> for PetActionEntry {
    fn from(packed: u32) -> Self {
        Self { packed }
    }
}

/// One decoded entry of `SMSG_PET_SPELLS`' trailing cooldown block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PetSpellCooldown {
    pub spell_id: u32,
    pub category: u16,
    pub spell_cd_ms: u32,
    /// The category remainder, possibly carrying [`PET_COOLDOWN_PERMANENT`] in its top bits.
    pub category_cd_ms: u32,
}

/// `SMSG_PET_SPELLS`: the pet action bar's entire state in one body.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PetSpells {
    /// The pet, possessed minion or charmed creature; zero means the bar is gone.
    pub pet_guid: u64,
    /// The pet's `CreatureFamily.dbc` id, which picks its talent tabs (`SetPet`, `0x5d6ab6`).
    pub family: u16,
    /// The possess or charm aura's remaining ms; 0 for a real pet.
    pub duration_ms: u32,
    /// The mode word, one dword as the client keeps it (`[0xc234c8]`): react state in byte 0,
    /// command state in byte 1, [`PET_STATE_VEHICLE`] at bit 27. Do not split it into bytes.
    pub state: u32,
    /// The ten bar slots, in bar order.
    pub bar: [PetActionEntry; PET_ACTION_SLOTS],
    /// The pet spellbook, packed like the bar (`AddPetSpell`, `0x5d6ca9`).
    pub spells: Vec<PetActionEntry>,
    pub cooldowns: Vec<PetSpellCooldown>,
}

impl PetSpells {
    /// Byte 0 of [`Self::state`], a [`PET_REACT_PASSIVE`]-family value.
    pub fn react_state(&self) -> u32 {
        self.state & 0xFF
    }
    /// `state >> 8`, unmasked like the client's compare (`0x5d5202`): with bit 27 set it matches
    /// no command state, so every command button goes dark. Masking it would lose that.
    pub fn command_state(&self) -> u32 {
        self.state >> 8
    }
    /// Bit 27: the bar is a vehicle's.
    pub fn vehicle(&self) -> bool {
        self.state & PET_STATE_VEHICLE != 0
    }
}

/// Read `SMSG_PET_SPELLS` as the 12340 handler does (`0x5d6b90`): `u64 petGuid`, then, for a
/// nonzero guid, `u16 family, u32 duration, u32 state, u32 bar[10], u8 count, u32
/// spells[count]` and the cooldown block. A zero guid is the teardown (`0x5d6c00`); any longer
/// body cut short is an error.
pub(super) fn read_pet_spells(r: &mut &[u8]) -> io::Result<PetSpells> {
    let pet_guid = read_u64_le(r)?;
    if pet_guid == 0 {
        return Ok(PetSpells::default());
    }
    let family = read_u16_le(r)?;
    let duration_ms = read_u32_le(r)?;
    let state = read_u32_le(r)?;

    let mut bar = [PetActionEntry::default(); PET_ACTION_SLOTS];
    for slot in &mut bar {
        *slot = read_u32_le(r)?.into();
    }

    let spell_count = read_u8(r)?;
    // A `u8` count with no tighter bound (`0x5d6c82`).
    let mut spells = Vec::with_capacity(capacity_hint(spell_count, usize::from(u8::MAX)));
    for _ in 0..spell_count {
        spells.push(read_u32_le(r)?.into());
    }

    let cooldowns = read_cooldown_block(r)?;

    Ok(PetSpells {
        pet_guid,
        family,
        duration_ms,
        state,
        bar,
        spells,
        cooldowns,
    })
}

/// The trailing cooldown block as 12340 reads it (`0x5d6cd3`-`0x5d6d0d`): `u8 count`, then
/// 14-byte entries `{u32 spell, u16 category, u32 cooldown, u32 categoryCooldown}`.
fn read_cooldown_block(r: &mut &[u8]) -> io::Result<Vec<PetSpellCooldown>> {
    let count = usize::from(read_u8(r)?);
    // `count` came off a `u8`; no tighter bound exists.
    let mut cooldowns = Vec::with_capacity(capacity_hint(count, usize::from(u8::MAX)));
    for _ in 0..count {
        cooldowns.push(PetSpellCooldown {
            spell_id: read_u32_le(r)?,
            category: read_u16_le(r)?,
            spell_cd_ms: read_u32_le(r)?,
            category_cd_ms: read_u32_le(r)?,
        });
    }
    Ok(cooldowns)
}

/// `SMSG_PET_MODE`: the state word without the bar, for a command or reaction change; 12340 takes
/// it only for the bar's own pet (`0x5d3630`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PetMode {
    pub pet_guid: u64,
    /// The same dword as [`PetSpells::state`], stored whole (`[0xc234c8]`).
    pub state: u32,
}

/// Read `SMSG_PET_MODE`: `u64 petGuid, u32 state` (`0x5d3630`).
pub(super) fn read_pet_mode(r: &mut impl Read) -> io::Result<PetMode> {
    Ok(PetMode {
        pet_guid: read_u64_le(r)?,
        state: read_u32_le(r)?,
    })
}

/// Read `SMSG_PET_ACTION_FEEDBACK` (`0x5d4da0`): one reason byte. After reasons 2 and 3 the
/// handler reads a `u32` spell for its cooldown refund, read and dropped here when sent.
pub(super) fn read_pet_action_feedback(r: &mut &[u8]) -> io::Result<u8> {
    let reason = read_u8(r)?;
    if matches!(reason, 2 | 3) && r.len() >= 4 {
        read_u32_le(r)?;
    }
    Ok(reason)
}

/// Read `SMSG_PET_TAME_FAILURE` (`0x802090`): one reason byte.
pub(super) fn read_pet_tame_failure(r: &mut impl Read) -> io::Result<u8> {
    read_u8(r)
}

/// Read `SMSG_PET_NAME_INVALID` as 12340's arm does (`0x6e2fb1`): the `u32` name code it hands
/// `DisplayNameError`, the refused name, a `u8` declension flag and, when set, five declined
/// forms. Only the code is kept; the declensions feed `PET_FORCE_NAME_DECLENSION`, which the
/// ruRU client alone asks for.
pub(super) fn read_pet_name_invalid(r: &mut impl Read) -> io::Result<u32> {
    let code = read_u32_le(r)?;
    read_cstring(r)?;
    skip_declined_names(r)?;
    Ok(code)
}

/// The declension block the pet-name packets share: a `u8` flag and, when set, five declined
/// forms of the name (`0x63532d`, `0x6e2fb1`).
fn skip_declined_names(r: &mut impl Read) -> io::Result<()> {
    if read_u8(r)? != 0 {
        for _ in 0..5 {
            read_cstring(r)?;
        }
    }
    Ok(())
}

/// Read `SMSG_PET_NAME_QUERY_RESPONSE` as 12340's handler does (`0x6352c0`): `u32 petNumber`,
/// the name, `u32 nameTimestamp`, then the declension block, read and dropped. An empty name is
/// the negative answer (`DBCache::DenyItem`, `0x635365`); the timestamp only ages the disk cache.
pub(super) fn read_pet_name_query_response(r: &mut impl Read) -> io::Result<(u32, String)> {
    let pet_number = read_u32_le(r)?;
    let name = read_cstring(r)?;
    let _name_timestamp = read_u32_le(r)?;
    skip_declined_names(r)?;
    Ok((pet_number, name))
}

/// The `GlobalStrings.lua` key filling `ERR_TAME_FAILED`'s `%s`: the reference's table
/// (`Spell_C_PetTameFailure`, `0x800390`) covers `1..=12`; anything else is the default arm.
pub fn pet_tame_failure_key(reason: u8) -> &'static str {
    match reason {
        1 => "PETTAME_INVALIDCREATURE",
        2 => "PETTAME_TOOMANY",
        3 => "PETTAME_CREATUREALREADYOWNED",
        4 => "PETTAME_NOTTAMEABLE",
        5 => "PETTAME_ANOTHERSUMMONACTIVE",
        6 => "PETTAME_UNITSCANTTAME",
        7 => "PETTAME_NOPETAVAILABLE",
        8 => "PETTAME_INTERNALERROR",
        9 => "PETTAME_TOOHIGHLEVEL",
        10 => "PETTAME_DEAD",
        11 => "PETTAME_NOTDEAD",
        12 => "PETTAME_CANTCONTROLEXOTIC",
        _ => "PETTAME_UNKNOWNERROR",
    }
}

/// Sound selector for an accepted order: AI reaction 1 (`0x716b90`).
pub const PET_TALK_ORDER: u32 = 0;
/// Sound selector for an attack order: AI reaction 2 (`0x716b90`).
pub const PET_TALK_ATTACK: u32 = 1;

/// Read `SMSG_PET_ACTION_SOUND`: `u64 petGuid, u32 selector` (`0x716b90`). The selector is a
/// `PET_TALK_*` value, not a `SoundEntries` id: the sound comes off the pet's
/// `CreatureSoundData` row.
pub(super) fn read_pet_action_sound(r: &mut impl Read) -> io::Result<(u64, u32)> {
    Ok((read_u64_le(r)?, read_u32_le(r)?))
}

/// Read `SMSG_PET_DISMISS_SOUND`: `u32 creatureModelDataId, f32 x, y, z` (`0x716c00`). The
/// handler's `+1.0` on `z` is a play detail, not applied here.
pub(super) fn read_pet_dismiss_sound(r: &mut impl Read) -> io::Result<(u32, Vector3d)> {
    Ok((read_u32_le(r)?, Vector3d::read(r)?))
}

/// Body of `CMSG_PET_ACTION` (opcode 373, `SendPetAction` `0x5d455e`): `u64` pet guid, the slot's
/// `u32` word echoed verbatim, `u64` target guid (0 when the action needs none).
pub fn pet_action(pet_guid: u64, packed: u32, target_guid: u64) -> Vec<u8> {
    let mut body = Vec::with_capacity(20);
    body.extend_from_slice(&pet_guid.to_le_bytes());
    body.extend_from_slice(&packed.to_le_bytes());
    body.extend_from_slice(&target_guid.to_le_bytes());
    body
}

/// Body of `CMSG_PET_CAST_SPELL` (opcode 496, `0x80b315`): the caster's `u64` guid, `castCount`,
/// `u32` spell, then the targets tail `CMSG_CAST_SPELL` shares (`0x80b351`).
pub fn pet_cast_spell(
    pet_guid: u64,
    cast_count: u8,
    spell_id: u32,
    target: super::UseItemTarget,
) -> Vec<u8> {
    let mut body = Vec::with_capacity(36);
    body.extend_from_slice(&pet_guid.to_le_bytes());
    body.push(cast_count);
    body.extend_from_slice(&spell_id.to_le_bytes());
    super::items::put_cast_targets(&mut body, target);
    body
}

/// Body of `CMSG_PET_STOP_ATTACK` (opcode 746, `0x5d4665`): the pet guid, sent by the Attack
/// button's second press.
pub fn pet_stop_attack(pet_guid: u64) -> Vec<u8> {
    pet_guid.to_le_bytes().to_vec()
}

/// Body of `CMSG_PET_CANCEL_AURA` (opcode 619, sender `0x803495`): `u64` pet guid, `u32` spell.
pub fn pet_cancel_aura(pet_guid: u64, spell_id: u32) -> Vec<u8> {
    let mut body = Vec::with_capacity(12);
    body.extend_from_slice(&pet_guid.to_le_bytes());
    body.extend_from_slice(&spell_id.to_le_bytes());
    body
}

/// Body of `CMSG_PET_SPELL_AUTOCAST` (opcode 755, sender `0x5d5f6e`): `u64` guid, `u32` spell,
/// `u8` state. The pet spellbook's autocast verb, with no reply; the bar's is [`pet_set_action`].
pub fn pet_spell_autocast(pet_guid: u64, spell_id: u32, enabled: bool) -> Vec<u8> {
    let mut body = Vec::with_capacity(13);
    body.extend_from_slice(&pet_guid.to_le_bytes());
    body.extend_from_slice(&spell_id.to_le_bytes());
    body.push(u8::from(enabled));
    body
}

/// Body of `CMSG_PET_SET_ACTION` (opcode 372): `u64` pet guid, then one or two
/// `(u32 position, u32 packed)` pairs, counted by body size alone, so one pair must not be padded.
/// It carries the bar's autocast toggle (one pair, bit 30 flipped, `0x5d5d96`) and its drag
/// (`0x5d5c52`), whose optional first pair relocates the target slot's occupant.
pub fn pet_set_action(pet_guid: u64, entries: &[(u32, u32)]) -> Vec<u8> {
    let mut body = Vec::with_capacity(8 + 8 * entries.len());
    body.extend_from_slice(&pet_guid.to_le_bytes());
    for &(position, packed) in entries {
        body.extend_from_slice(&position.to_le_bytes());
        body.extend_from_slice(&packed.to_le_bytes());
    }
    body
}

/// Body of `CMSG_PET_ABANDON` (opcode 374, `PetAbandon` `0x5d4913`): the pet guid.
pub fn pet_abandon(pet_guid: u64) -> Vec<u8> {
    pet_guid.to_le_bytes().to_vec()
}

/// Body of `CMSG_PET_RENAME` (opcode 375) as 12340's sender `0x5d4a00` builds it: the pet guid,
/// the name NUL-terminated, then a `u8` declension flag, 0 here; the five declined forms that
/// follow a 1 are the ruRU client's alone. The server refuses a bad name with
/// `SMSG_PET_NAME_INVALID` rather than truncating.
pub fn pet_rename(pet_guid: u64, name: &str) -> Vec<u8> {
    let mut body = Vec::with_capacity(10 + name.len());
    body.extend_from_slice(&pet_guid.to_le_bytes());
    body.extend_from_slice(name.as_bytes());
    body.push(0);
    body.push(0);
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 12340 reads `SMSG_CAST_FAILED`'s body here (`0x806c30`): count, spell, reason, args.
    #[test]
    fn pet_cast_failed_reads_the_cast_failed_body() {
        use crate::events::{decode, SessionEvent};
        let parse = |body: &[u8]| {
            decode(
                crate::messages::parse_server(crate::messages::opcode::SMSG_PET_CAST_FAILED, body)
                    .unwrap(),
            )
        };
        // Growl (2649), wire 0x66 REQUIRES_SPELL_FOCUS with focus 3: shared 0x5e.
        assert!(matches!(
            parse(&[4, 0x59, 0x0a, 0, 0, 0x66, 3, 0, 0, 0]).as_slice(),
            [SessionEvent::PetCastFailed {
                spell_id: 2649,
                reason: 0x5e,
                arg: Some(3),
            }]
        ));
        // No argument: wire 0x0c BAD_TARGETS, shared 0x0a.
        assert!(matches!(
            parse(&[4, 0x59, 0x0a, 0, 0, 0x0c]).as_slice(),
            [SessionEvent::PetCastFailed {
                spell_id: 2649,
                reason: 0x0a,
                arg: None,
            }]
        ));
    }

    /// A hunter pet's `SMSG_PET_SPELLS` in the 12340 layout, with one spell and one cooldown.
    fn pet_spells_body() -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&0xF140_0000_0000_002Au64.to_le_bytes()); // pet guid
        b.extend_from_slice(&1u16.to_le_bytes()); // family (wolf)
        b.extend_from_slice(&0u32.to_le_bytes()); // duration (0 = a real pet)
        b.push(PET_REACT_DEFENSIVE as u8);
        b.push(PET_COMMAND_FOLLOW as u8);
        b.push(0);
        b.push(0); // enabled
                   // The bar is CharmInfo::InitPetActionBar's default, Claw 3010 autocasting.
        for packed in [
            PET_COMMAND_ATTACK | (u32::from(PET_ACT_COMMAND) << 24),
            PET_COMMAND_FOLLOW | (u32::from(PET_ACT_COMMAND) << 24),
            PET_COMMAND_STAY | (u32::from(PET_ACT_COMMAND) << 24),
            3010 | (u32::from(PET_ACT_ENABLED) << 24),
            u32::from(PET_ACT_DISABLED) << 24, // empty
            u32::from(PET_ACT_DISABLED) << 24, // empty
            u32::from(PET_ACT_DISABLED) << 24, // empty
            PET_REACT_AGGRESSIVE | (u32::from(PET_ACT_REACTION) << 24),
            PET_REACT_DEFENSIVE | (u32::from(PET_ACT_REACTION) << 24),
            PET_REACT_PASSIVE | (u32::from(PET_ACT_REACTION) << 24),
        ] {
            b.extend_from_slice(&packed.to_le_bytes());
        }
        b.push(1); // spell count
        b.extend_from_slice(&(3010u32 | (u32::from(PET_ACT_ENABLED) << 24)).to_le_bytes());
        // The cooldown block: u8 count, 14-byte entries.
        b.push(1);
        b.extend_from_slice(&3010u32.to_le_bytes());
        b.extend_from_slice(&0u16.to_le_bytes()); // category
        b.extend_from_slice(&4_500u32.to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes());
        b
    }

    #[test]
    fn pet_spells_golden() {
        let body = pet_spells_body();
        let mut r = &body[..];
        let p = read_pet_spells(&mut r).unwrap();
        assert!(r.is_empty(), "the body is fully consumed");

        assert_eq!(p.pet_guid, 0xF140_0000_0000_002A);
        assert_eq!(p.duration_ms, 0);
        assert_eq!(p.react_state(), PET_REACT_DEFENSIVE);
        assert_eq!(p.command_state(), PET_COMMAND_FOLLOW);
        assert!(!p.vehicle());
        assert_eq!(p.family, 1);

        assert_eq!(p.bar[0].kind(), PET_ACT_COMMAND);
        assert_eq!(p.bar[0].action(), PET_COMMAND_ATTACK);
        assert!(!p.bar[0].is_spell());
        assert_eq!(p.bar[9].kind(), PET_ACT_REACTION);
        assert_eq!(p.bar[9].action(), PET_REACT_PASSIVE);

        // The server sent 0xC1; the client's type is 1, with autocast in bits 31/30.
        assert_eq!(p.bar[3].kind(), PET_TYPE_SPELL);
        assert_eq!(p.bar[3].action(), 3010);
        assert!(p.bar[3].is_spell() && !p.bar[3].is_empty());
        assert!(p.bar[3].autocast_allowed() && p.bar[3].autocast_on());

        // A server's unused slot (0, ACT_DISABLED) is not empty; it takes the spell branch.
        assert!(!p.bar[4].is_empty());
        assert!(p.bar[4].is_spell() && p.bar[4].action() == 0);
        assert!(
            PetActionEntry::default().is_empty(),
            "zero IS the empty word"
        );

        assert_eq!(p.spells.len(), 1);
        assert_eq!(p.spells[0].action(), 3010);
        assert_eq!(
            p.cooldowns,
            vec![PetSpellCooldown {
                spell_id: 3010,
                category: 0,
                spell_cd_ms: 4_500,
                category_cd_ms: 0,
            }]
        );
    }

    /// No cooldowns: one count byte, fully consumed.
    /// Freeze at a ground point: the pet's full guid, count, spell, flags 0, mask `0x40`, the
    /// transport's empty packed guid and three floats.
    #[test]
    fn pet_cast_spell_dest_body_golden() {
        let body = pet_cast_spell(
            0xF140_0000_0000_0007,
            3,
            33395,
            super::super::UseItemTarget::Dest([1.0, 2.0, 3.0]),
        );
        let mut want = vec![0x07, 0, 0, 0, 0, 0, 0x40, 0xF1, 3, 0x73, 0x82, 0, 0, 0];
        want.extend_from_slice(&0x40u32.to_le_bytes());
        want.push(0);
        for c in [1.0f32, 2.0, 3.0] {
            want.extend_from_slice(&c.to_le_bytes());
        }
        assert_eq!(body, want);
    }

    #[test]
    fn an_empty_cooldown_block_is_one_byte() {
        let mut r = &[0u8][..];
        assert!(read_cooldown_block(&mut r).unwrap().is_empty());
        assert!(r.is_empty());
    }

    /// 12340 keeps 24 action bits (`0x5d6c65`): a pet spell past `0xffff` survives.
    #[test]
    fn the_action_is_24_bits() {
        let e = PetActionEntry::from(0x1_0A3C | (u32::from(PET_ACT_ENABLED) << 24));
        assert_eq!(e.action(), 0x1_0A3C);
        assert!(e.is_spell());
    }

    /// A zero-guid body is the only signal that the bar is gone, so it must not be an error.
    #[test]
    fn the_guid_only_body_is_the_teardown() {
        let body = 0u64.to_le_bytes();
        let mut r = &body[..];
        let p = read_pet_spells(&mut r).unwrap();
        assert_eq!(p, PetSpells::default());
        assert_eq!(p.pet_guid, 0);
    }

    /// Cut after the first bar slot: an error, not a half-read bar.
    #[test]
    fn a_truncated_body_errors() {
        let body = pet_spells_body();
        let mut r = &body[..20];
        assert!(read_pet_spells(&mut r).is_err());
    }

    #[test]
    fn pet_mode_and_the_client_verbs_golden() {
        // SMSG_PET_MODE: a vehicle's fourth state byte 0x8 is the client's bit 27.
        let mut body = 0x2Au64.to_le_bytes().to_vec();
        body.extend_from_slice(&[2, 0, 0, 0x8]);
        let mut r = &body[..];
        let mode = read_pet_mode(&mut r).unwrap();
        assert_eq!(mode.pet_guid, 0x2A);
        assert_eq!(mode.state, 0x0800_0002);
        assert_eq!(mode.state & PET_STATE_VEHICLE, PET_STATE_VEHICLE);

        let packed = 3010 | (u32::from(PET_ACT_ENABLED) << 24);
        assert_eq!(
            pet_action(0x2A, packed, 0x99),
            [
                0x2Au64.to_le_bytes().to_vec(),
                packed.to_le_bytes().to_vec(),
                0x99u64.to_le_bytes().to_vec(),
            ]
            .concat()
        );

        assert_eq!(pet_stop_attack(0x2A), 0x2Au64.to_le_bytes());

        assert_eq!(
            pet_cancel_aura(0x2A, 2645),
            [
                0x2Au64.to_le_bytes().to_vec(),
                2645u32.to_le_bytes().to_vec()
            ]
            .concat()
        );
        assert_eq!(pet_cancel_aura(0x2A, 2645).len(), 12);

        // CMSG_PET_SET_ACTION: the server counts entries by body size, so 16 bytes or exactly 24.
        assert_eq!(pet_set_action(0x2A, &[(3, packed)]).len(), 16);
        assert_eq!(pet_set_action(0x2A, &[(3, packed), (4, 0)]).len(), 24);

        assert_eq!(pet_abandon(0x2A), 0x2Au64.to_le_bytes());

        // CMSG_PET_RENAME: guid, a NUL-terminated name, then the declension flag.
        assert_eq!(
            pet_rename(0x2A, "Bruce"),
            [&0x2Au64.to_le_bytes()[..], b"Bruce\0\0"].concat()
        );
        assert_eq!(
            pet_rename(0x2A, "").len(),
            10,
            "an empty name is still framed"
        );
    }

    /// `SMSG_PET_NAME_QUERY_RESPONSE` through the parse arm, fully consumed in each shape.
    #[test]
    fn pet_name_query_response_reads_the_declension_block() {
        use crate::messages::{opcode, parse_server, ServerPacket};
        let body = |name: &[u8], declined: Option<[&[u8]; 5]>| {
            let mut b = 7u32.to_le_bytes().to_vec();
            b.extend_from_slice(name);
            b.push(0);
            b.extend_from_slice(&0x6500_0000u32.to_le_bytes()); // timestamp
            match declined {
                None => b.push(0),
                Some(forms) => {
                    b.push(1);
                    for f in forms {
                        b.extend_from_slice(f);
                        b.push(0);
                    }
                }
            }
            b
        };
        let parse = |b: &[u8]| {
            let mut r = b;
            let n = read_pet_name_query_response(&mut r).unwrap();
            assert!(r.is_empty(), "the body is fully consumed");
            assert!(matches!(
                parse_server(opcode::SMSG_PET_NAME_QUERY_RESPONSE, b).unwrap(),
                ServerPacket::PetNameQueryResponse { pet_number: 7, .. }
            ));
            n
        };

        assert_eq!(parse(&body(b"Rex", None)), (7, "Rex".into()));
        assert_eq!(
            parse(&body(
                b"Rex",
                Some([b"Rexa", b"Rexu", b"Rexe", b"Rexom", b"Rexy"])
            )),
            (7, "Rex".into())
        );
        assert_eq!(parse(&body(b"", None)), (7, String::new()));

        // A body without the declension flag is an error rather than a silent success.
        let short = &body(b"Rex", None)[..12];
        assert!(read_pet_name_query_response(&mut &short[..]).is_err());
    }

    #[test]
    fn the_autocast_flip_moves_only_bit_30() {
        let on = PetActionEntry::from(3010 | (u32::from(PET_ACT_ENABLED) << 24));
        assert!(on.autocast_allowed() && on.autocast_on());

        let off = on.with_autocast(false);
        assert_eq!(off.packed, 3010 | (u32::from(PET_ACT_DISABLED) << 24));
        assert_eq!(off.action(), 3010);
        assert_eq!(off.kind(), PET_TYPE_SPELL, "the TYPE never moves");
        assert!(off.autocast_allowed() && !off.autocast_on());
        assert_eq!(off.with_autocast(true), on);

        // Bit 31 is the server's to grant; flipping bit 30 on a passive spell does not forge it.
        let passive = PetActionEntry::from(3010 | (u32::from(PET_ACT_PASSIVE) << 24));
        assert!(!passive.with_autocast(true).autocast_allowed());
    }

    #[test]
    fn the_type_mask_collapses_the_spell_states() {
        for act in [PET_ACT_PASSIVE, PET_ACT_DISABLED, PET_ACT_ENABLED] {
            let e = PetActionEntry::from(3010 | (u32::from(act) << 24));
            assert_eq!(e.kind(), PET_TYPE_SPELL, "{act:#04x} masks to 1");
            assert!(e.is_spell());
        }
        assert_eq!(
            PetActionEntry::from(u32::from(PET_ACT_COMMAND) << 24).kind(),
            PET_ACT_COMMAND,
            "the token types are below the mask, so they survive it unchanged"
        );
        assert_eq!(
            PetActionEntry::from(u32::from(PET_ACT_REACTION) << 24).kind(),
            PET_ACT_REACTION
        );

        // Types 8-17 take the spell branch too; 2-5 and 18+ are neither a spell nor a token.
        for kind in [8u32, 17] {
            assert!(PetActionEntry::from(3010 | (kind << 24)).is_spell());
        }
        for kind in [2u32, 5, 18, 0x33] {
            assert!(!PetActionEntry::from(3010 | (kind << 24)).is_spell());
        }
        let odd = PetActionEntry::from(3010 | (0x33u32 << 24));
        assert!(odd.kind() != PET_ACT_COMMAND && odd.kind() != PET_ACT_REACTION);
    }
}

/// `SMSG_PET_UNLEARN_CONFIRM`: the trainer's guid and the unlearn cost in copper, which the
/// reference latches before firing `CONFIRM_PET_UNLEARN(cost)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PetUnlearnConfirm {
    pub trainer: u64,
    pub cost: u32,
}

/// Parse `SMSG_PET_UNLEARN_CONFIRM`: `u64 trainerGuid`, `u32 costCopper`.
pub(super) fn read_pet_unlearn_confirm(
    r: &mut impl std::io::Read,
) -> std::io::Result<PetUnlearnConfirm> {
    Ok(PetUnlearnConfirm {
        trainer: crate::wire::read_u64_le(r)?,
        cost: crate::wire::read_u32_le(r)?,
    })
}

/// Body of `CMSG_PET_UNLEARN`: the latched trainer guid.
pub fn pet_unlearn(trainer: u64) -> Vec<u8> {
    trainer.to_le_bytes().to_vec()
}
