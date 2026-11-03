//! The player summon: `SMSG_SUMMON_REQUEST` (0x2ab) and its answer, `CMSG_SUMMON_RESPONSE` (0x2ac),
//! which `ConfirmSummon` sends agreeing and `CancelSummon` declining.

use std::io;

use crate::wire::{read_u32_le, read_u64_le};

/// `SMSG_SUMMON_REQUEST`: someone is asking to pull us to them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SummonRequest {
    /// Echoed in [`summon_response`]; `GetSummonConfirmSummoner()` names it from the name cache.
    pub summoner: u64,
    /// The summoner's `AreaTable.dbc` zone id, not ours and not a map id.
    pub zone: u32,
    /// Ms until the server auto-declines (vmangos `MAX_PLAYER_SUMMON_DELAY`, two minutes); the
    /// client counts down from arrival and closes the dialog at zero.
    pub delay_ms: u32,
}

/// Read `SMSG_SUMMON_REQUEST`: guid, zone, delay.
pub(super) fn read_summon_request(r: &mut &[u8]) -> io::Result<SummonRequest> {
    Ok(SummonRequest {
        summoner: read_u64_le(r)?,
        zone: read_u32_le(r)?,
        delay_ms: read_u32_le(r)?,
    })
}

/// Body of `CMSG_SUMMON_RESPONSE`: the summoner's guid, then the agree byte, 1 from `ConfirmSummon`
/// (`0x51b0dc`) and 0 from `CancelSummon` (`0x51b16a`).
pub fn summon_response(summoner_guid: u64, agree: bool) -> Vec<u8> {
    let mut body = summoner_guid.to_le_bytes().to_vec();
    body.push(u8::from(agree));
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_request_body_is_guid_then_zone_then_delay() {
        let mut bytes = 0xF130_0000_0001_2345u64.to_le_bytes().to_vec();
        bytes.extend_from_slice(&1519u32.to_le_bytes()); // Stormwind City
        bytes.extend_from_slice(&120_000u32.to_le_bytes()); // two minutes
        assert_eq!(
            read_summon_request(&mut &bytes[..]).unwrap(),
            SummonRequest {
                summoner: 0xF130_0000_0001_2345,
                zone: 1519,
                delay_ms: 120_000,
            }
        );
    }

    #[test]
    fn a_short_request_body_is_an_error() {
        let bytes = [0u8; 15];
        assert!(read_summon_request(&mut &bytes[..]).is_err());
    }

    #[test]
    fn the_response_body_is_the_guid_then_the_agree_byte() {
        assert_eq!(
            summon_response(0x0000_0000_0000_2a01, true),
            vec![0x01, 0x2a, 0, 0, 0, 0, 0, 0, 1]
        );
        assert_eq!(
            summon_response(0x0000_0000_0000_2a01, false),
            vec![0x01, 0x2a, 0, 0, 0, 0, 0, 0, 0]
        );
    }
}
