//! The GM ticket family behind the Help window, as the 12340 client speaks it: the create, edit,
//! get, delete, queue-status and resolve requests, and the eight answers its one handler
//! (`0x5ad240`) reads. The get, delete, status and resolve requests are empty.

use std::io;

use crate::wire::{read_cstring, read_f32_le, read_u32_le, read_u8};

/// `SMSG_GMTICKET_GETTICKET`'s status when a ticket follows (`0x5ad30a`); any other is no ticket.
pub const GMTICKET_STATUS_HASTEXT: u32 = 0x06;

/// `SMSG_GMTICKET_GETTICKET`'s database-error status, the one that shows `ERR_TICKET_DB_ERROR`
/// (`0x5ad311`).
pub const GMTICKET_STATUS_DBERROR: u32 = 0x00;

/// `SMSG_GMTICKET_GETTICKET`'s ordinary no-ticket status: a 4-byte body.
pub const GMTICKET_STATUS_DEFAULT: u32 = 0x0A;

/// The queue is accepting tickets (`GMTICKET_QUEUE_STATUS_ENABLED`).
pub const GMTICKET_QUEUE_ENABLED: i32 = 1;

/// `SMSG_GMTICKET_DELETETICKET`'s success (`0x5ad40f`); anything else is a database error.
pub const GMTICKET_RESPONSE_TICKET_DELETED: u32 = 9;

/// The player's open ticket from `SMSG_GMTICKET_GETTICKET`, in wire order (`0x5ad33b`-`0x5ad3b4`).
/// The `f32`s are days, and a negative oldest or update figure means unknown.
#[derive(Debug, Clone, PartialEq)]
pub struct GmTicket {
    /// The server's ticket id, the client's `m_ticketID` (`[0xc1e8d0]`) that gates edit and delete.
    pub id: u32,
    /// What the player typed, read into a 2000-byte buffer (`0x5ad351`).
    pub text: String,
    /// The byte the client fires as `UPDATE_TICKET`'s first argument.
    pub category: u8,
    /// Days since the ticket was last modified, clamped to 0 as the client does (`0x5ad37a`).
    pub ticket_age: f32,
    /// Days since the realm's oldest open ticket was modified, 0 when none, negative when unknown.
    pub oldest_ticket_age: f32,
    /// Age of the oldest-ticket figure in days; negative or over 0.042 (about an hour) is stale.
    pub update_time: f32,
    /// 0 unassigned, 1 assigned to a GM, 2 escalated.
    pub assigned_to_gm: u8,
    /// 1 once a GM has actually opened the ticket, 0 before that.
    pub opened_by_gm: u8,
}

/// `SMSG_GMRESPONSE_RECEIVED` (`0x5ad4ef`): a GM's answer to a ticket, which the client keeps as
/// `m_responseID` and `m_ticketID` and fires as `GMRESPONSE_RECEIVED(text, response)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GmResponse {
    pub response_id: u32,
    pub ticket_id: u32,
    /// The ticket's own text.
    pub text: String,
    /// The answer: four cstrings of up to 3999 bytes, joined (`0x5ad540`).
    pub response: String,
}

/// `CMSG_GMTICKET_CREATE` as `0x5ac8f0` writes it: `u32` map, the position, the text, `u32` 17
/// when the player wants a reply and 1 when not (`0x5ac9d0`), `u8` need-more-help, then the chat
/// transcript's line count, per-line ages and size. Deviation: no transcript (count 0, size 0),
/// where the client packs its last 60 chat lines with zlib; servers accept the empty one.
pub fn gm_ticket_create(
    map: u32,
    pos: [f32; 3],
    text: &str,
    need_response: bool,
    more_help: bool,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + 12 + text.len() + 1 + 4 + 1 + 8);
    out.extend_from_slice(&map.to_le_bytes());
    for c in pos {
        out.extend_from_slice(&c.to_le_bytes());
    }
    out.extend_from_slice(text.as_bytes());
    out.push(0);
    let response: u32 = if need_response { 17 } else { 1 };
    out.extend_from_slice(&response.to_le_bytes());
    out.push(u8::from(more_help));
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out
}

/// `CMSG_GMTICKET_UPDATETEXT` as `0x5acb50` writes it: the text alone.
pub fn gm_ticket_updatetext(text: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() + 1);
    out.extend_from_slice(text.as_bytes());
    out.push(0);
    out
}

/// Read the `u32` answer shared by create, update text, delete and the status push: 1 already
/// exists, 2 created, 3 create failed, 4 updated, 5 update failed, 9 deleted.
pub(super) fn read_gm_ticket_response(r: &mut &[u8]) -> io::Result<u32> {
    read_u32_le(r)
}

/// Read `SMSG_GMTICKETSYSTEMSTATUS`, 1 when the queue takes tickets and 0 when not. Signed, so
/// `HelpFrame`'s `GMTICKET_QUEUE_STATUS_DISABLED` (-1) arm is reachable.
pub(super) fn read_gm_ticket_system_status(r: &mut &[u8]) -> io::Result<i32> {
    Ok(read_u32_le(r)? as i32)
}

/// Read `SMSG_GMTICKET_GETTICKET`: the status, then the ticket only for
/// [`GMTICKET_STATUS_HASTEXT`]. The status stays, as 0 shows an error where the rest are silent.
pub(super) fn read_gm_ticket(r: &mut &[u8]) -> io::Result<(u32, Option<GmTicket>)> {
    let status = read_u32_le(r)?;
    if status != GMTICKET_STATUS_HASTEXT {
        return Ok((status, None));
    }
    let id = read_u32_le(r)?;
    let text = read_cstring(r)?;
    let category = read_u8(r)?;
    // `fcom` against 0 with the age kept only on `0 <= age`, so a NaN clamps too.
    let age = read_f32_le(r)?;
    let ticket_age = if age >= 0.0 { age } else { 0.0 };
    Ok((
        status,
        Some(GmTicket {
            id,
            text,
            category,
            ticket_age,
            oldest_ticket_age: read_f32_le(r)?,
            update_time: read_f32_le(r)?,
            assigned_to_gm: read_u8(r)?,
            opened_by_gm: read_u8(r)?,
        }),
    ))
}

/// Read `SMSG_GMRESPONSE_RECEIVED`: the two ids, the ticket's text, then the four answer pieces.
pub(super) fn read_gm_response(r: &mut &[u8]) -> io::Result<GmResponse> {
    let response_id = read_u32_le(r)?;
    let ticket_id = read_u32_le(r)?;
    let text = read_cstring(r)?;
    let mut response = String::new();
    for _ in 0..4 {
        response.push_str(&read_cstring(r)?);
    }
    Ok(GmResponse {
        response_id,
        ticket_id,
        text,
        response,
    })
}

/// Read `SMSG_GMRESPONSE_STATUS_UPDATE` (`0x5ad4b2`): one byte, nonzero to offer the survey.
pub(super) fn read_gm_response_status(r: &mut &[u8]) -> io::Result<bool> {
    Ok(read_u8(r)? != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The field order and widths of 12340's sender (`0x5ac8f0`).
    #[test]
    fn the_create_body_is_map_position_text_response_flag_more_help_and_an_empty_transcript() {
        let body = gm_ticket_create(1, [-8949.95, -132.493, 83.5312], "My sword.", true, false);
        let mut want = 1u32.to_le_bytes().to_vec();
        want.extend_from_slice(&(-8949.95f32).to_le_bytes());
        want.extend_from_slice(&(-132.493f32).to_le_bytes());
        want.extend_from_slice(&(83.5312f32).to_le_bytes());
        want.extend_from_slice(b"My sword.\0");
        want.extend_from_slice(&17u32.to_le_bytes());
        want.push(0);
        want.extend_from_slice(&0u32.to_le_bytes());
        want.extend_from_slice(&0u32.to_le_bytes());
        assert_eq!(body, want);

        let quiet = gm_ticket_create(0, [0.0; 3], "x", false, true);
        assert_eq!(&quiet[18..22], &1u32.to_le_bytes(), "no reply wanted is 1");
        assert_eq!(quiet[22], 1, "the need-more-help follow-up");
    }

    /// The text alone, no category byte (`0x5acb50`).
    #[test]
    fn the_updatetext_body_is_the_text_alone() {
        assert_eq!(gm_ticket_updatetext("Still stuck."), b"Still stuck.\0");
    }

    fn held_ticket(age: f32) -> Vec<u8> {
        let mut bytes = GMTICKET_STATUS_HASTEXT.to_le_bytes().to_vec();
        bytes.extend_from_slice(&77u32.to_le_bytes());
        bytes.extend_from_slice(b"Stuck in a rock.\0");
        bytes.push(1);
        bytes.extend_from_slice(&age.to_le_bytes());
        bytes.extend_from_slice(&2.5f32.to_le_bytes());
        bytes.extend_from_slice(&0.01f32.to_le_bytes());
        bytes.push(2);
        bytes.push(1);
        bytes
    }

    /// The status-6 body leads with the ticket id (`0x5ad34c`).
    #[test]
    fn a_held_ticket_decodes_with_its_id_ahead_of_the_text() {
        let bytes = held_ticket(0.25);
        let mut r = &bytes[..];
        assert_eq!(
            read_gm_ticket(&mut r).unwrap(),
            (
                GMTICKET_STATUS_HASTEXT,
                Some(GmTicket {
                    id: 77,
                    text: "Stuck in a rock.".to_string(),
                    category: 1,
                    ticket_age: 0.25,
                    oldest_ticket_age: 2.5,
                    update_time: 0.01,
                    assigned_to_gm: 2,
                    opened_by_gm: 1,
                })
            )
        );
        assert!(r.is_empty(), "the whole body is read");
    }

    /// `0x5ad37a`: a negative age reads as 0; the other two figures stay signed.
    #[test]
    fn a_negative_ticket_age_clamps_to_zero() {
        let bytes = held_ticket(-3.0);
        let (_, ticket) = read_gm_ticket(&mut &bytes[..]).unwrap();
        assert_eq!(ticket.unwrap().ticket_age, 0.0);
    }

    /// Every status but 6 is no ticket, the error one included; the status survives.
    #[test]
    fn the_no_ticket_answers_are_four_bytes_and_keep_their_status() {
        for status in [GMTICKET_STATUS_DEFAULT, GMTICKET_STATUS_DBERROR, 3] {
            let bytes = status.to_le_bytes();
            assert_eq!(read_gm_ticket(&mut &bytes[..]).unwrap(), (status, None));
        }
    }

    #[test]
    fn the_queue_status_is_signed_so_all_ones_reads_as_minus_one() {
        assert_eq!(
            read_gm_ticket_system_status(&mut &0xFFFF_FFFFu32.to_le_bytes()[..]).unwrap(),
            -1
        );
        assert_eq!(
            read_gm_ticket_system_status(&mut &1u32.to_le_bytes()[..]).unwrap(),
            GMTICKET_QUEUE_ENABLED
        );
    }

    /// The answer arrives as four pieces, which the client joins with nothing between them.
    #[test]
    fn a_gm_response_joins_its_four_pieces() {
        let mut bytes = 1u32.to_le_bytes().to_vec();
        bytes.extend_from_slice(&77u32.to_le_bytes());
        bytes.extend_from_slice(b"Stuck.\0");
        bytes.extend_from_slice(b"Try \0");
        bytes.extend_from_slice(b"Hearthing.\0");
        bytes.extend_from_slice(b"\0\0");
        let mut r = &bytes[..];
        assert_eq!(
            read_gm_response(&mut r).unwrap(),
            GmResponse {
                response_id: 1,
                ticket_id: 77,
                text: "Stuck.".into(),
                response: "Try Hearthing.".into(),
            }
        );
        assert!(r.is_empty());
        assert!(read_gm_response_status(&mut &[1u8][..]).unwrap());
        assert!(!read_gm_response_status(&mut &[0u8][..]).unwrap());
    }
}
