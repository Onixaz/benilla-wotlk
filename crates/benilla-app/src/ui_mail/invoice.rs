//! The auction house's mail: a notice's subject and body are colon-separated text the client
//! parses back, the subject in `ParseInboxSubject` (`0x56f520`) and the body in
//! `GetInboxInvoiceInfo` (`0x56e9b0`).
//!
//! The subject `<itemEntry>:<randomProperty>:<resultCode>[:<value>…]` names the outcome and is
//! displayed through the matching `AUCTION_*_MAIL_SUBJECT` string, never raw. Won, sold and
//! pending-sale notices are invoices, whose numbers are in the body.

/// The subject's `resultCode`; the reference keys its subject strings on `0..=6` (`0x56f69f`).
pub(crate) mod auction_mail {
    pub(crate) const OUTBID: i32 = 0;
    pub(crate) const WON: i32 = 1;
    pub(crate) const SOLD: i32 = 2;
    pub(crate) const EXPIRED: i32 = 3;
    pub(crate) const CANCELLED_TO_BIDDER: i32 = 4;
    pub(crate) const CANCELLED: i32 = 5;
    /// A sale whose money is still on its way.
    pub(crate) const SALE_PENDING: i32 = 6;
}

/// A parsed auction-mail subject.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AuctionSubject {
    pub(crate) entry: u32,
    pub(crate) random_property: i32,
    pub(crate) result: i32,
}

impl AuctionSubject {
    /// Won, sold and a pending sale carry an invoice (`0x56f720`).
    pub(crate) fn is_invoice(&self) -> bool {
        matches!(
            self.result,
            auction_mail::WON | auction_mail::SOLD | auction_mail::SALE_PENDING
        )
    }

    /// `GetInboxInvoiceInfo`'s first value: `"seller"`, `"seller_temp_invoice"` or `"buyer"`.
    pub(crate) fn invoice_kind(&self) -> &'static str {
        match self.result {
            auction_mail::SOLD => "seller",
            auction_mail::SALE_PENDING => "seller_temp_invoice",
            _ => "buyer",
        }
    }
}

/// `SStrToInt`: leading whitespace, an optional sign, then digits up to the first non-digit; 0
/// when there are none.
fn str_to_int(s: &str) -> i32 {
    let s = s.trim_start();
    let (neg, digits) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let mut n: i32 = 0;
    for b in digits.bytes().take_while(u8::is_ascii_digit) {
        n = n.wrapping_mul(10).wrapping_add(i32::from(b - b'0'));
    }
    if neg {
        n.wrapping_neg()
    } else {
        n
    }
}

/// `ParseInboxSubject` (`0x56f520`): the item from the leading number, then each later field after
/// the next colon, a missing one reading 0.
pub(crate) fn parse_subject(subject: &str) -> AuctionSubject {
    let mut fields = subject.split(':');
    let entry = str_to_int(fields.next().unwrap_or_default()) as u32;
    let random_property = fields.next().map_or(0, str_to_int);
    let result = fields.next().map_or(0, str_to_int);
    AuctionSubject {
        entry,
        random_property,
        result,
    }
}

/// The `AUCTION_*_MAIL_SUBJECT` key a result code displays under (`0x56f69f`); the two cancel
/// codes share one, and an unknown code shows an empty subject.
pub(crate) fn subject_key(result: i32) -> Option<&'static str> {
    Some(match result {
        auction_mail::OUTBID => "AUCTION_OUTBID_MAIL_SUBJECT",
        auction_mail::WON => "AUCTION_WON_MAIL_SUBJECT",
        auction_mail::SOLD => "AUCTION_SOLD_MAIL_SUBJECT",
        auction_mail::EXPIRED => "AUCTION_EXPIRED_MAIL_SUBJECT",
        auction_mail::CANCELLED_TO_BIDDER | auction_mail::CANCELLED => {
            "AUCTION_REMOVED_MAIL_SUBJECT"
        }
        auction_mail::SALE_PENDING => "AUCTION_INVOICE_MAIL_SUBJECT",
        _ => return None,
    })
}

/// The numbers an invoice body carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct InvoiceNumbers {
    /// The counterparty's guid, hex right-aligned in a 16-wide field.
    pub(crate) player_guid: u64,
    pub(crate) bid: u32,
    pub(crate) buyout: u32,
    pub(crate) deposit: u32,
    pub(crate) consignment: u32,
    pub(crate) money_delay: u32,
    /// The packed time the pending sale's money is mailed.
    pub(crate) eta: u32,
}

/// A body read with the reference's `sscanf` formats: `%16I64X`, then `fields` `%d`s, each after
/// a colon (`0x56eaac`-`0x56eb0a`). The scan stops at the first mismatch and every field it did
/// not reach stays 0, so a body with more fields than the format reads its prefix.
pub(crate) fn parse_body(body: &str, fields: usize) -> InvoiceNumbers {
    let mut nums = [0u32; 6];
    let rest = body.trim_start();
    let hex_len = rest
        .bytes()
        .take(16)
        .take_while(u8::is_ascii_hexdigit)
        .count();
    let Ok(player_guid) = u64::from_str_radix(&rest[..hex_len], 16) else {
        return InvoiceNumbers::default();
    };
    let mut rest = &rest[hex_len..];
    for slot in nums.iter_mut().take(fields) {
        let Some(after) = rest.strip_prefix(':') else {
            break;
        };
        let t = after.trim_start();
        let sign = usize::from(t.starts_with(['-', '+']));
        let len = sign + t[sign..].bytes().take_while(u8::is_ascii_digit).count();
        if len == sign {
            break;
        }
        *slot = str_to_int(&t[..len]) as u32;
        rest = &t[len..];
    }
    let [bid, buyout, deposit, consignment, money_delay, eta] = nums;
    InvoiceNumbers {
        player_guid,
        bid,
        buyout,
        deposit,
        consignment,
        money_delay,
        eta,
    }
}

/// How many `%d`s each invoice kind reads after the guid: a seller four, a pending sale six, a
/// buyer two (`0x56eaac`-`0x56eb0a`).
pub(crate) fn body_fields(result: i32) -> usize {
    match result {
        auction_mail::SOLD => 4,
        auction_mail::SALE_PENDING => 6,
        _ => 2,
    }
}

/// `WowDecodeTime`'s hour and minute of a packed time: minutes in the low 6 bits, the hour in the
/// next 5, an all-ones field reading -1.
pub(crate) fn decode_hour_minute(packed: u32) -> (i32, i32) {
    let minute = packed & 0x3f;
    let hour = (packed >> 6) & 0x1f;
    (
        if hour == 0x1f { -1 } else { hour as i32 },
        if minute == 0x3f { -1 } else { minute as i32 },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Subjects as AzerothCore writes them: entry, property, result, auction id, count.
    #[test]
    fn the_subject_names_the_item_and_the_outcome() {
        let sold = parse_subject("2589:0:2:1001:5");
        assert_eq!((sold.entry, sold.result), (2589, auction_mail::SOLD));
        assert!(sold.is_invoice());
        assert_eq!(sold.invoice_kind(), "seller");
        assert_eq!(subject_key(sold.result), Some("AUCTION_SOLD_MAIL_SUBJECT"));

        let pending = parse_subject("2589:0:6:1001:5");
        assert_eq!(pending.invoice_kind(), "seller_temp_invoice");
        assert_eq!(
            subject_key(pending.result),
            Some("AUCTION_INVOICE_MAIL_SUBJECT")
        );

        let expired = parse_subject("2589:-7:3");
        assert_eq!(expired.random_property, -7);
        assert!(!expired.is_invoice());
        assert_eq!(
            subject_key(auction_mail::CANCELLED_TO_BIDDER),
            subject_key(auction_mail::CANCELLED)
        );
        assert_eq!(subject_key(9), None);
    }

    #[test]
    fn a_short_or_odd_subject_reads_zero_for_what_is_missing() {
        assert_eq!(
            parse_subject("4428"),
            AuctionSubject {
                entry: 4428,
                random_property: 0,
                result: 0
            }
        );
        assert_eq!(parse_subject("Hi there").entry, 0);
    }

    /// AzerothCore pads the guid to 16 with spaces and always writes seven fields.
    #[test]
    fn every_kind_reads_its_prefix_of_the_body() {
        let body = "              6C:10000:10000:25:500:3600:1234567";
        let buyer = parse_body(body, body_fields(auction_mail::WON));
        assert_eq!(buyer.player_guid, 0x6C, "hex, not decimal");
        assert_eq!((buyer.bid, buyer.buyout, buyer.deposit), (10000, 10000, 0));

        let seller = parse_body(body, body_fields(auction_mail::SOLD));
        assert_eq!((seller.deposit, seller.consignment), (25, 500));
        assert_eq!(seller.money_delay, 0, "the seller format stops at five");

        let pending = parse_body(body, body_fields(auction_mail::SALE_PENDING));
        assert_eq!((pending.money_delay, pending.eta), (3600, 1234567));

        // A body cut short keeps what it reached.
        let short = parse_body("6C:900", body_fields(auction_mail::SOLD));
        assert_eq!((short.bid, short.buyout), (900, 0));
    }

    #[test]
    fn the_packed_time_gives_hour_and_minute() {
        assert_eq!(decode_hour_minute((14 << 6) | 35), (14, 35));
        assert_eq!(decode_hour_minute(0x7ff), (-1, -1));
    }
}
