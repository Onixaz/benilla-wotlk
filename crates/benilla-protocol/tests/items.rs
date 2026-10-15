//! The item wire: the query request and its full template reply, `CMSG_USE_ITEM`'s target forms,
//! equip and open, and the inventory-change refusals.

mod common;

use benilla_protocol::events::{decode, SessionEvent};
use benilla_protocol::messages;
use common::hx;

/// A miss is the lone entry with the top bit set (vmangos's unknown-entry branch).
#[test]
fn item_query_response_miss() {
    let fail = messages::parse_server(
        messages::opcode::SMSG_ITEM_QUERY_SINGLE_RESPONSE,
        &hx("d2040080"),
    )
    .unwrap();
    match decode(fail).pop().unwrap() {
        SessionEvent::ItemTemplate { entry, info, .. } => {
            assert_eq!((entry, info), (1234, None));
        }
        _ => panic!("item query miss event"),
    }
}
