//! Stable-master messages: a hunter's current pet plus up to four bought stable slots. The gossip
//! stable option makes the server send `MSG_LIST_STABLED_PETS`; every other verb gets only a
//! `SMSG_STABLE_RESULT` byte, never a fresh list. Only hunter pets are listed, so a warlock sees an
//! empty stable.

use std::io;

use crate::wire::{capacity_hint, read_cstring, read_u32_le, read_u64_le, read_u8};

/// One `MSG_LIST_STABLED_PETS` row, the current pet or a stabled one. It has no display id or
/// family; those come from a creature query on the entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StabledPet {
    /// The server's pet id; [`unstable_pet`] and [`stable_swap_pet`] take this, not a slot.
    pub pet_number: u32,
    /// A `creature_template` entry, not a display id.
    pub creature_entry: u32,
    pub level: u32,
    /// The name the hunter gave it, not the template's.
    pub name: String,
    /// The row's state byte; see [`Self::is_current`].
    pub flags: u8,
}

impl StabledPet {
    /// The current pet's row: bit `0x2` clear (`0x5a129a`); every other row is a stabled pet.
    pub fn is_current(&self) -> bool {
        self.flags & 0x2 == 0
    }
}

/// `SMSG_STABLE_RESULT` codes, as the handler `0x5a17f0` switches on them (table `0x5a181a`);
/// 2-7 show nothing.
pub mod stable_result {
    /// Not enough money for the next slot: `ERR_NOT_ENOUGH_MONEY` (`0x5a1821`).
    pub const ERR_MONEY: u8 = 0x01;
    /// Any other refusal; the reference shows nothing for it.
    pub const ERR_STABLE: u8 = 0x06;
    /// The current pet went into a stable slot; the list is asked again (`0x5a1886`).
    pub const SUCCESS_STABLE: u8 = 0x08;
    /// A stabled pet is now current, after an unstable or a swap; the list is asked again.
    pub const SUCCESS_UNSTABLE: u8 = 0x09;
    /// A slot was bought: the count moves (`0x5a1835`), then the list is asked again.
    pub const SUCCESS_BUY_SLOT: u8 = 0x0A;
    /// Fires `PET_STABLE_UPDATE` (`0x5a18ff`).
    pub const UPDATE: u8 = 0x0B;
    /// The exotic-pet refusal: the tame-failure line with reason 12 (`0x5a1916`).
    pub const ERR_EXOTIC: u8 = 0x0C;
}

/// Body of `MSG_LIST_STABLED_PETS` from the client (`Npc.cpp:51-54`): the refresh, needed after
/// every successful verb because the server never resends the list.
pub fn list_stabled_pets(npc_guid: u64) -> Vec<u8> {
    npc_guid.to_le_bytes().to_vec()
}

/// Body of `CMSG_STABLE_PET` (`Npc.cpp:56-59`). It carries no slot: the server takes the first
/// free bought slot or refuses (`HandleStablePet`, `NPCHandler.cpp:609-655`).
pub fn stable_pet(npc_guid: u64) -> Vec<u8> {
    npc_guid.to_le_bytes().to_vec()
}

/// Body of `CMSG_UNSTABLE_PET` (`Npc.cpp:61-65`): makes the pet current. Refused while the player
/// has any pet, even one out of range (`NPCHandler.cpp:657-702`); swapping is [`stable_swap_pet`].
pub fn unstable_pet(npc_guid: u64, pet_number: u32) -> Vec<u8> {
    let mut body = Vec::with_capacity(12);
    body.extend_from_slice(&npc_guid.to_le_bytes());
    body.extend_from_slice(&pet_number.to_le_bytes());
    body
}

/// Body of `CMSG_BUY_STABLE_SLOT` (`Npc.cpp:67-70`): buys the next slot at its
/// `StableSlotPrices.dbc` price, which the client reads itself (`NPCHandler.cpp:704-729`).
pub fn buy_stable_slot(npc_guid: u64) -> Vec<u8> {
    npc_guid.to_le_bytes().to_vec()
}

/// Body of `CMSG_STABLE_SWAP_PET` (`Npc.cpp:72-76`): the current pet and the named one trade
/// places in one step (`NPCHandler.cpp:735-789`).
pub fn stable_swap_pet(npc_guid: u64, pet_number: u32) -> Vec<u8> {
    let mut body = Vec::with_capacity(12);
    body.extend_from_slice(&npc_guid.to_le_bytes());
    body.extend_from_slice(&pet_number.to_le_bytes());
    body
}

// No `CMSG_STABLE_REVIVE_PET` (0x0274) verb: vmangos's handler is empty (`NPCHandler.cpp:731`).

/// Read `MSG_LIST_STABLED_PETS` from the server (`0x5a11a0`): the stable master, `u8` row count,
/// `u8` bought-slot count (0..=4, not occupied), then per row `u32` pet number, `u32` entry, `u32`
/// level, the name and a `u8` flags byte. Rows stay in wire order: the current pet's may be
/// missing, and the stabled ones are listed packed, not by slot.
pub(super) fn read_list_stabled_pets(r: &mut &[u8]) -> io::Result<(u64, u8, Vec<StabledPet>)> {
    let npc = read_u64_le(r)?;
    let num_pets = read_u8(r)?;
    let num_stable_slots = read_u8(r)?;
    // The current pet plus the four stable slots.
    let mut pets = Vec::with_capacity(capacity_hint(num_pets, 1 + 4));
    for _ in 0..num_pets {
        // Struct-literal fields evaluate top-to-bottom, so this reads in wire order.
        pets.push(StabledPet {
            pet_number: read_u32_le(r)?,
            creature_entry: read_u32_le(r)?,
            level: read_u32_le(r)?,
            name: read_cstring(r)?,
            flags: read_u8(r)?,
        });
    }
    Ok((npc, num_stable_slots, pets))
}

/// Read `SMSG_STABLE_RESULT` (`0x5a17fe`): one [`stable_result`] byte.
pub(super) fn read_stable_result(r: &mut &[u8]) -> io::Result<u8> {
    read_u8(r)
}
