//! The account-data wire, 3.3.5a's: the server's copies of the eight `WTF\Account\` slots
//! (handlers registered at the account-data system's start, `0x6b9900`).

use std::io::{self, Read, Write};

use crate::wire::{read_u32_le, read_u64_le, read_u8};

/// `SMSG_ACCOUNT_DATA_TIMES` (`0x6b94c0`): the server's clock and a time per slot in `mask`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountDataTimes {
    pub server_time: u32,
    /// Read by the reference only to clamp a global slot's server time up to its local one when 0.
    pub flag: u8,
    pub mask: u32,
    /// The time of each slot whose bit is set, by slot.
    pub times: [Option<u32>; 8],
}

/// Read `SMSG_ACCOUNT_DATA_TIMES`: `u32 server time, u8 flag, u32 mask`, then a `u32` per set bit
/// of the mask's low byte in slot order; any other length is refused whole, as `0x6b94c0` drops it.
pub fn read_account_data_times(r: &mut &[u8]) -> io::Result<AccountDataTimes> {
    let server_time = read_u32_le(r)?;
    let flag = read_u8(r)?;
    let mask = read_u32_le(r)?;
    if r.len() != 4 * (mask & 0xff).count_ones() as usize {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "account data times: the times do not match the mask",
        ));
    }
    let mut times = [None; 8];
    for (slot, time) in times.iter_mut().enumerate() {
        if mask & (1 << slot) != 0 {
            *time = Some(read_u32_le(r)?);
        }
    }
    Ok(AccountDataTimes {
        server_time,
        flag,
        mask,
        times,
    })
}

/// `SMSG_UPDATE_ACCOUNT_DATA` (`0x6b9730`): one slot's bytes, still compressed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountDataUpdate {
    /// The character the slot belongs to, 0 for a global slot.
    pub guid: u64,
    pub slot: u32,
    pub time: u32,
    /// The inflated length; 0 is an empty slot, and what follows is then ignored.
    pub size: u32,
    pub compressed: Vec<u8>,
}

/// Read `SMSG_UPDATE_ACCOUNT_DATA`: `u64 guid, u32 slot, u32 time, u32 size`, then the zlib stream
/// to the end of the packet.
pub fn read_update_account_data(r: &mut &[u8]) -> io::Result<AccountDataUpdate> {
    let guid = read_u64_le(r)?;
    let slot = read_u32_le(r)?;
    let time = read_u32_le(r)?;
    let size = read_u32_le(r)?;
    let compressed = std::mem::take(r).to_vec();
    Ok(AccountDataUpdate {
        guid,
        slot,
        time,
        size,
        compressed,
    })
}

impl AccountDataUpdate {
    /// The slot's bytes, `size` long; `None` when the stream does not inflate to them, which the
    /// reference answers with an empty file.
    pub fn inflate(&self) -> Option<Vec<u8>> {
        if self.size == 0 {
            return Some(Vec::new());
        }
        let mut out = Vec::with_capacity(self.size as usize);
        flate2::read::ZlibDecoder::new(self.compressed.as_slice())
            .take(u64::from(self.size))
            .read_to_end(&mut out)
            .ok()?;
        (out.len() == self.size as usize).then_some(out)
    }
}

/// `CMSG_REQUEST_ACCOUNT_DATA` body (the download, `0x6b92c0`): the slot.
pub fn request_account_data(slot: u32) -> Vec<u8> {
    slot.to_le_bytes().to_vec()
}

/// `CMSG_UPDATE_ACCOUNT_DATA` body (the upload, `0x6b9100`): `u32 slot, u32 time, u32 size`, then
/// the bytes deflated at level 1 (`0x778040`), nothing for an empty slot.
pub fn update_account_data(slot: u32, time: u32, bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(12 + bytes.len() / 2);
    out.extend_from_slice(&slot.to_le_bytes());
    out.extend_from_slice(&time.to_le_bytes());
    out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    if !bytes.is_empty() {
        let mut z = flate2::write::ZlibEncoder::new(out, flate2::Compression::new(1));
        // A `Vec` sink cannot fail.
        let _ = z.write_all(bytes);
        out = z.finish().unwrap_or_default();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_times_carry_one_u32_per_set_bit() {
        let mut body = vec![];
        body.extend_from_slice(&1_800_000_000u32.to_le_bytes());
        body.push(1);
        body.extend_from_slice(&0x15u32.to_le_bytes());
        for t in [10u32, 0, 30] {
            body.extend_from_slice(&t.to_le_bytes());
        }
        let times = read_account_data_times(&mut body.as_slice()).unwrap();
        assert_eq!(times.server_time, 1_800_000_000);
        assert_eq!(
            times.times,
            [Some(10), None, Some(0), None, Some(30), None, None, None]
        );
        assert!(
            read_account_data_times(&mut &body[..body.len() - 4]).is_err(),
            "a short run is refused whole"
        );
    }

    #[test]
    fn an_upload_inflates_back_to_its_bytes() {
        let file = b"SET a \"1\"\nSET b \"2\"\n";
        let body = update_account_data(1, 77, file);
        assert_eq!(&body[..4], &1u32.to_le_bytes());
        assert_eq!(&body[8..12], &(file.len() as u32).to_le_bytes());
        // The server echoes the stream back after a guid.
        let mut echo = 0x42u64.to_le_bytes().to_vec();
        echo.extend_from_slice(&body);
        let update = read_update_account_data(&mut echo.as_slice()).unwrap();
        assert_eq!((update.guid, update.slot, update.time), (0x42, 1, 77));
        assert_eq!(update.inflate().as_deref(), Some(&file[..]));
        assert_eq!(
            update_account_data(4, 1, b"").len(),
            12,
            "an empty slot has no stream"
        );
    }
}
