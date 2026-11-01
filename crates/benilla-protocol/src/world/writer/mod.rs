//! The world session's write half: [`WorldWriter`] and one method per thing the player can do.
//! Each family module mirrors its namesake in `crate::messages`, which builds the bodies.

use std::net::TcpStream;

use super::cipher::Encrypter;
use anyhow::Result;

use super::send_packet;

mod account_data;
mod action_bar;
mod area_trigger;
mod arena_team;
mod attack;
mod auction;
mod bank;
mod barber;
mod battlefield;
mod binder;
mod calendar;
mod channel;
mod chat;
mod death;
mod duel;
mod gameobject;
mod gm_ticket;
mod gossip;
mod group;
mod guild;
mod guild_bank;
mod instance;
mod items;
mod lfg;
mod lifecycle;
mod loot;
mod mail;
mod meeting_stone;
mod names;
mod notices;
mod pet;
mod petition;
mod player_flags;
mod pose;
mod progression;
mod pvp;
mod quest;
mod reputation;
mod selection;
mod self_movement;
mod skills;
mod social;
mod spells;
mod stable;
mod summon;
mod tabard;
mod taxi;
mod trade;
mod trainer;
mod tutorial;
mod vendor;

/// Write half of a split [`WorldSession`](super::WorldSession): a cloned socket and the encrypter.
/// Movement sends need the active player confirmed as mover first
/// ([`WorldSession::set_active_mover`](super::WorldSession::set_active_mover)). Positions are raw
/// WoW yards; `orientation` is radians.
pub struct WorldWriter {
    pub(super) stream: TcpStream,
    pub(super) encrypter: Encrypter,
    /// The character's faction language, set at login from its race, which a chat send naming no
    /// language carries. vmangos drops the whole message, dot-commands included, when the
    /// character does not know the language.
    pub(super) chat_language: u32,
    /// The `castCount` each `CMSG_CAST_SPELL` carries.
    pub(super) cast_counter: crate::messages::CastCounter,
    /// The unit every movement packet names, set by [`Self::set_active_mover`].
    pub(super) mover: u64,
}

impl WorldWriter {
    /// Frame, encrypt and write one packet: the sole write path every verb goes through.
    fn send(&mut self, opcode: u16, body: &[u8]) -> Result<()> {
        send_packet(&mut self.stream, Some(&mut self.encrypter), opcode, body)
    }
}
