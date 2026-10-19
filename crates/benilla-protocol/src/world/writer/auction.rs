//! The auction house sends of `CGAuctionHouse`; each carries the auctioneer guid. Sell, bid and
//! cancel answer `SMSG_AUCTION_COMMAND_RESULT`.

use anyhow::Result;

use crate::messages::{self, opcode};

use super::WorldWriter;

impl WorldWriter {
    /// Greet an auctioneer; the reply, carrying the `AuctionHouse.dbc` id, opens the window.
    pub fn auction_hello(&mut self, auctioneer: u64) -> Result<()> {
        self.send(
            opcode::MSG_AUCTION_HELLO,
            &messages::auction_hello(auctioneer),
        )
    }

    /// Ask a Browse page with the Browse list's sort criteria (`QueryAuctionItems`, `0x59bcf0`).
    pub fn auction_list_items(
        &mut self,
        auctioneer: u64,
        query: &messages::AuctionListQuery,
    ) -> Result<()> {
        self.send(
            opcode::CMSG_AUCTION_LIST_ITEMS,
            &messages::auction_list_items(auctioneer, query),
        )
    }

    /// Ask a page of our own auctions (`CMSG_AUCTION_LIST_OWNER_ITEMS`).
    pub fn auction_list_owner_items(&mut self, auctioneer: u64, list_from: u32) -> Result<()> {
        self.send(
            opcode::CMSG_AUCTION_LIST_OWNER_ITEMS,
            &messages::auction_list_owner_items(auctioneer, list_from),
        )
    }

    /// Ask the sold auctions still in the mail (`CMSG_AUCTION_LIST_PENDING_SALES`).
    pub fn auction_list_pending_sales(&mut self, auctioneer: u64) -> Result<()> {
        self.send(
            opcode::CMSG_AUCTION_LIST_PENDING_SALES,
            &messages::auction_list_pending_sales(auctioneer),
        )
    }

    /// Ask the Bid tab (`CMSG_AUCTION_LIST_BIDDER_ITEMS`) with the outbid ids of this house.
    pub fn auction_list_bidder_items(
        &mut self,
        auctioneer: u64,
        list_from: u32,
        auction_ids: &[u32],
    ) -> Result<()> {
        self.send(
            opcode::CMSG_AUCTION_LIST_BIDDER_ITEMS,
            &messages::auction_list_bidder_items(auctioneer, list_from, auction_ids),
        )
    }

    /// List one auction made of `items` `(guid, count)`; `etime_minutes` is 720, 1440 or 2880.
    pub fn auction_sell_item(
        &mut self,
        auctioneer: u64,
        items: &[(u64, u32)],
        bid: u32,
        buyout: u32,
        etime_minutes: u32,
    ) -> Result<()> {
        self.send(
            opcode::CMSG_AUCTION_SELL_ITEM,
            &messages::auction_sell_item(auctioneer, items, bid, buyout, etime_minutes),
        )
    }

    /// Bid on an auction; a `price` equal to a nonzero buyout is the buyout.
    pub fn auction_place_bid(
        &mut self,
        auctioneer: u64,
        auction_id: u32,
        price: u32,
    ) -> Result<()> {
        self.send(
            opcode::CMSG_AUCTION_PLACE_BID,
            &messages::auction_place_bid(auctioneer, auction_id, price),
        )
    }

    /// Cancel one of our auctions.
    pub fn auction_remove_item(&mut self, auctioneer: u64, auction_id: u32) -> Result<()> {
        self.send(
            opcode::CMSG_AUCTION_REMOVE_ITEM,
            &messages::auction_remove_item(auctioneer, auction_id),
        )
    }
}
