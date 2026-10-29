//! The duel family: challenge, countdown, bounds, completion and the winner line. The 1.12 client's
//! handlers are registered; the two bounds messages have empty bodies.

use std::io::{self, Read};

use crate::wire::{read_cstring, read_u32_le, read_u64_le, read_u8};

/// `SMSG_DUEL_REQUESTED`, a duel challenge sent to both parties (vmangos `EffectDuel`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DuelRequested {
    /// The duel flag game object (entry 21680) between the players: the bounds anchor, the
    /// `PLAYER_DUEL_ARBITER` value, and the guid echoed on accept and cancel.
    pub arbiter: u64,
    /// Who challenged; when it is us, the 1.12 client shows "You have requested a duel." and
    /// auto-accepts.
    pub challenger: u64,
}

/// Read `SMSG_DUEL_REQUESTED`: two full 8-byte guids, arbiter first.
pub fn read_duel_requested(r: &mut impl Read) -> io::Result<DuelRequested> {
    Ok(DuelRequested {
        arbiter: read_u64_le(r)?,
        challenger: read_u64_le(r)?,
    })
}

/// Read `SMSG_DUEL_COMPLETE`'s `started` flag; false means the duel ended before it began, which
/// the 1.12 client reports as `ERR_DUEL_CANCELLED`.
pub fn read_duel_complete(r: &mut impl Read) -> io::Result<bool> {
    Ok(read_u8(r)? != 0)
}

/// `SMSG_DUEL_WINNER`, the outcome line, broadcast around the loser (`Player::DuelComplete`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuelWinner {
    /// True when the loser fled or forfeited (`DUEL_WINNER_RETREAT`), false for a knockout
    /// (`DUEL_WINNER_KNOCKOUT`); the 1.12 client picks the line by it.
    pub fled: bool,
    /// The winner's name, `%1$s`.
    pub winner: String,
    /// The loser's name, `%2$s`.
    pub loser: String,
}

/// Read `SMSG_DUEL_WINNER`: `u8` flag, then winner and loser cstrings in that order.
pub fn read_duel_winner(r: &mut impl Read) -> io::Result<DuelWinner> {
    Ok(DuelWinner {
        fled: read_u8(r)? != 0,
        winner: read_cstring(r)?,
        loser: read_cstring(r)?,
    })
}

/// Read `SMSG_DUEL_COUNTDOWN` as whole seconds: the wire carries ms (vmangos sends 3000) and the
/// 1.12 client truncates them to seconds.
pub fn read_duel_countdown(r: &mut impl Read) -> io::Result<u32> {
    Ok(read_u32_le(r)? / 1000)
}

/// `CMSG_DUEL_ACCEPTED` body: the arbiter guid from the request, as the 1.12 client sends it;
/// vmangos ignores it.
pub fn duel_accepted(arbiter: u64) -> Vec<u8> {
    arbiter.to_le_bytes().to_vec()
}

/// `CMSG_DUEL_CANCELLED` body: the arbiter guid. The server reads it by duel state as a decline, a
/// countdown cancel, or once started a forfeit that hands the opponent the win.
pub fn duel_cancelled(arbiter: u64) -> Vec<u8> {
    arbiter.to_le_bytes().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duel_requested_is_arbiter_then_challenger() {
        let body = [
            0xEF, 0xBE, 0xAD, 0xDE, 0x00, 0x00, 0x00, 0xF1, // arbiter
            0x07, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // challenger
        ];
        assert_eq!(
            read_duel_requested(&mut &body[..]).unwrap(),
            DuelRequested {
                arbiter: 0xF100_0000_DEAD_BEEF,
                challenger: 7,
            }
        );
    }

    #[test]
    fn duel_complete_started_byte() {
        assert!(!read_duel_complete(&mut &[0u8][..]).unwrap());
        assert!(read_duel_complete(&mut &[1u8][..]).unwrap());
    }

    #[test]
    fn duel_winner_flag_then_two_names() {
        let mut body = vec![1u8];
        body.extend_from_slice(b"Onerogue\0");
        body.extend_from_slice(b"Twomage\0");
        assert_eq!(
            read_duel_winner(&mut &body[..]).unwrap(),
            DuelWinner {
                fled: true,
                winner: "Onerogue".into(),
                loser: "Twomage".into(),
            }
        );
    }

    #[test]
    fn duel_countdown_is_milliseconds_truncated_to_seconds() {
        assert_eq!(
            read_duel_countdown(&mut &3000u32.to_le_bytes()[..]).unwrap(),
            3
        );
        assert_eq!(
            read_duel_countdown(&mut &3999u32.to_le_bytes()[..]).unwrap(),
            3
        );
        assert_eq!(
            read_duel_countdown(&mut &0u32.to_le_bytes()[..]).unwrap(),
            0
        );
    }

    /// The bytes push.
    #[test]
    fn accept_and_cancel_carry_the_arbiter_guid() {
        let guid = 0x0000_00F1_DEAD_BEEFu64;
        assert_eq!(duel_accepted(guid), guid.to_le_bytes());
        assert_eq!(duel_cancelled(guid), guid.to_le_bytes());
    }
}
