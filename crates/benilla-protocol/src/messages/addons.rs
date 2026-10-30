//! The addon-info block ending `CMSG_AUTH_SESSION`, written by the reference after the 20-byte
//! proof: a `u32` uncompressed size, then a zlib (RFC1950) stream to the end of the packet.
//! Uncompressed, it is one record per addon whose `.toc` sets `## Secure:` non-zero, enabled or
//! not, with no count or trailer: `CString` name, `u8` flags (`.pub` byte 0), `u32` CRC-32 of the
//! 256 modulus bytes (`.pub` bytes 1..=256), `u32` CRC-32 of the `.url` string (0 if none).
//!
//! With no secure addons the client appends nothing. Both emulators refuse a zero size: vmangos
//! then skips `SMSG_ADDON_INFO` (`WorldSocket.cpp:447`), cmangos-classic kicks the session
//! (`WorldSocket.cpp:562`).
//!
//! 3.3.5a frames the same records with a `u32` count before them and a `u32` after them, the time
//! of the last banned-addon list the server sent (AzerothCore `WorldSession::ReadAddonsInfo`).

/// CRC-32 of the stock Blizzard public-key modulus, the emulators' "standard addon CRC"; a server
/// seeing it does not send the 256-byte modulus back in `SMSG_ADDON_INFO`.
pub const STANDARD_MODULUS_CRC: u32 = 0x4C1C_776D;

/// The addons `SMSG_ADDON_INFO` hides: record i answers `sent[i]` (the reply carries no names), and
/// status 2 makes the reference set `[rec+0x29] = 1`, dropping it from Lua.
pub fn hidden_from_reply(statuses: &[u8], sent: &[SecureAddon]) -> Vec<String> {
    statuses
        .iter()
        .zip(sent)
        .filter(|(status, _)| **status == 2)
        .map(|(_, addon)| addon.name.to_string())
        .collect()
}

/// One record of the addon block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SecureAddon<'a> {
    /// The addon's folder name (`Blizzard_AuctionUI`), sent as a `CString`.
    pub name: &'a str,
    /// Byte 0 of the `.pub`: 1 for a stock signature, 0 for none usable. A server may write another
    /// back in `SMSG_ADDON_INFO`, which the reference saves to the `.pub` and echoes next logon.
    pub flags: u8,
    /// CRC-32 of the `.pub`'s 256 modulus bytes; 0 when `flags` is 0.
    pub modulus_crc: u32,
    /// CRC-32 of the addon's `.url` string. `0` for every stock addon (none ship one).
    pub url_crc: u32,
}

impl SecureAddon<'_> {
    /// A stock signed built-in, the shape all twelve take on an unmodified install.
    const fn stock(name: &str) -> SecureAddon<'_> {
        SecureAddon {
            name,
            flags: 1,
            modulus_crc: STANDARD_MODULUS_CRC,
            url_crc: 0,
        }
    }
}

/// The twelve `Blizzard_*` built-ins a stock 1.12.1 install reports, in the client's order
/// (ascending ASCII). Deviation: a fixed list, not read off the install, because no third-party
/// addon sets `## Secure:`, so the two differ only where the `.pub` files were altered.
pub const STOCK_SECURE_ADDONS: [SecureAddon<'static>; 12] = [
    SecureAddon::stock("Blizzard_AuctionUI"),
    SecureAddon::stock("Blizzard_BattlefieldMinimap"),
    SecureAddon::stock("Blizzard_BindingUI"),
    SecureAddon::stock("Blizzard_CombatText"),
    SecureAddon::stock("Blizzard_CraftUI"),
    SecureAddon::stock("Blizzard_GMSurveyUI"),
    SecureAddon::stock("Blizzard_InspectUI"),
    SecureAddon::stock("Blizzard_MacroUI"),
    SecureAddon::stock("Blizzard_RaidUI"),
    SecureAddon::stock("Blizzard_TalentUI"),
    SecureAddon::stock("Blizzard_TradeSkillUI"),
    SecureAddon::stock("Blizzard_TrainerUI"),
];

/// The 23 `Blizzard_*` built-ins of a stock 3.3.5a install, each with a stock-signed `.pub`
/// (`Interface\AddOns\Blizzard_*\*.pub`: byte 0 is 1, the modulus CRC [`STANDARD_MODULUS_CRC`]),
/// in ascending ASCII order as 1.12.1 sends its own.
pub const STOCK_SECURE_ADDONS_WOTLK: [SecureAddon<'static>; 23] = [
    SecureAddon::stock("Blizzard_AchievementUI"),
    SecureAddon::stock("Blizzard_ArenaUI"),
    SecureAddon::stock("Blizzard_AuctionUI"),
    SecureAddon::stock("Blizzard_BarbershopUI"),
    SecureAddon::stock("Blizzard_BattlefieldMinimap"),
    SecureAddon::stock("Blizzard_BindingUI"),
    SecureAddon::stock("Blizzard_Calendar"),
    SecureAddon::stock("Blizzard_CombatLog"),
    SecureAddon::stock("Blizzard_CombatText"),
    SecureAddon::stock("Blizzard_DebugTools"),
    SecureAddon::stock("Blizzard_GMChatUI"),
    SecureAddon::stock("Blizzard_GMSurveyUI"),
    SecureAddon::stock("Blizzard_GlyphUI"),
    SecureAddon::stock("Blizzard_GuildBankUI"),
    SecureAddon::stock("Blizzard_InspectUI"),
    SecureAddon::stock("Blizzard_ItemSocketingUI"),
    SecureAddon::stock("Blizzard_MacroUI"),
    SecureAddon::stock("Blizzard_RaidUI"),
    SecureAddon::stock("Blizzard_TalentUI"),
    SecureAddon::stock("Blizzard_TimeManager"),
    SecureAddon::stock("Blizzard_TokenUI"),
    SecureAddon::stock("Blizzard_TradeSkillUI"),
    SecureAddon::stock("Blizzard_TrainerUI"),
];

/// The uncompressed addon buffer: the records concatenated, nothing else.
pub fn addon_block(addons: &[SecureAddon]) -> Vec<u8> {
    let mut out = Vec::with_capacity(addons.iter().map(|a| a.name.len() + 10).sum());
    for addon in addons {
        out.extend_from_slice(addon.name.as_bytes());
        out.push(0);
        out.push(addon.flags);
        out.extend_from_slice(&addon.modulus_crc.to_le_bytes());
        out.extend_from_slice(&addon.url_crc.to_le_bytes());
    }
    out
}

/// The block on the wire: `u32` size and the zlib stream, or nothing when there are no addons.
pub fn addon_tail(addons: &[SecureAddon]) -> Vec<u8> {
    if addons.is_empty() {
        return Vec::new();
    }
    compressed(&addon_block(addons))
}

/// 3.3.5a's block on the wire: `u32` size, then the zlib stream of the count, the records and
/// `last_banned_addon_time`.
pub fn addon_tail_wotlk(addons: &[SecureAddon], last_banned_addon_time: u32) -> Vec<u8> {
    let mut plain = (addons.len() as u32).to_le_bytes().to_vec();
    plain.extend_from_slice(&addon_block(addons));
    plain.extend_from_slice(&last_banned_addon_time.to_le_bytes());
    compressed(&plain)
}

/// `u32` uncompressed size, then `plain` as one zlib stream.
fn compressed(plain: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(plain.len() / 2 + 8);
    out.extend_from_slice(&(plain.len() as u32).to_le_bytes());
    let mut encoder =
        flate2::write::ZlibEncoder::new(out, flate2::Compression::default() /* level 6 */);
    std::io::Write::write_all(&mut encoder, plain).expect("zlib write to a Vec cannot fail");
    encoder.finish().expect("zlib finish on a Vec cannot fail")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The block has retail's 342-byte length and parses as twelve sorted, stock-signed records.
    #[test]
    fn stock_block_has_the_retail_size_and_twelve_sorted_stock_records() {
        let plain = addon_block(&STOCK_SECURE_ADDONS);
        assert_eq!(plain.len(), 342, "retail sent 342 uncompressed bytes");

        // Parse it as a server does; landing exactly on the end proves no count and no trailer.
        let mut rest = &plain[..];
        let mut seen = Vec::new();
        while !rest.is_empty() {
            let nul = rest
                .iter()
                .position(|&b| b == 0)
                .expect("record has a name");
            let name = std::str::from_utf8(&rest[..nul]).expect("name is utf-8");
            rest = &rest[nul + 1..];
            let (flags, tail) = rest.split_first().expect("record has flags");
            let modulus_crc = u32::from_le_bytes(tail[0..4].try_into().unwrap());
            let url_crc = u32::from_le_bytes(tail[4..8].try_into().unwrap());
            assert_eq!(*flags, 1, "{name} is a stock signed addon");
            assert_eq!(modulus_crc, STANDARD_MODULUS_CRC, "{name} modulus crc");
            assert_eq!(url_crc, 0, "{name} ships no .url");
            seen.push(name.to_string());
            rest = &tail[8..];
        }
        assert_eq!(seen.len(), 12);
        assert!(seen.iter().all(|n| n.starts_with("Blizzard_")));
        let mut sorted = seen.clone();
        sorted.sort();
        assert_eq!(
            seen, sorted,
            "the client sends them in ascending ASCII order"
        );
    }

    /// `flate2`'s default is the reference's `Z_DEFAULT_COMPRESSION`, hence the retail 130 bytes.
    #[test]
    fn stock_tail_is_size_plus_zlib() {
        let tail = addon_tail(&STOCK_SECURE_ADDONS);
        assert_eq!(
            u32::from_le_bytes(tail[0..4].try_into().unwrap()),
            342,
            "size prefix is the uncompressed length"
        );
        assert_eq!(tail[4], 0x78, "RFC1950 zlib framing, not raw deflate");
        assert_eq!(tail.len(), 4 + 130, "retail compressed to 130 bytes");
    }

    #[test]
    fn no_secure_addons_appends_nothing() {
        assert!(addon_tail(&[]).is_empty());
    }

    /// Inflated, the 3.3.5a block is count, the 1.12.1 record shape, then the trailing time.
    #[test]
    fn the_wotlk_block_frames_the_records_with_a_count_and_a_time() {
        let tail = addon_tail_wotlk(&STOCK_SECURE_ADDONS_WOTLK, 0x0102_0304);
        let size = u32::from_le_bytes(tail[0..4].try_into().unwrap()) as usize;
        let mut plain = Vec::new();
        std::io::Read::read_to_end(&mut flate2::read::ZlibDecoder::new(&tail[4..]), &mut plain)
            .unwrap();
        assert_eq!(plain.len(), size);
        assert_eq!(&plain[..4], &23u32.to_le_bytes());
        assert_eq!(
            &plain[4..size - 4],
            &addon_block(&STOCK_SECURE_ADDONS_WOTLK)[..]
        );
        assert_eq!(&plain[size - 4..], &0x0102_0304u32.to_le_bytes());

        let names: Vec<_> = STOCK_SECURE_ADDONS_WOTLK.iter().map(|a| a.name).collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);
    }
}
