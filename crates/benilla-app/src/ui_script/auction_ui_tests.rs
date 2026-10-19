//! The stock 3.3.5a auction window (`Blizzard_AuctionUI`) on the production load, over the model
//! the app pushes: the class tree from the install, a session's lists and a bag of Linen Cloth.

use std::collections::HashMap;

use benilla_ui::script::{
    AuctionCategory, AuctionHighBidder, AuctionItemRow, AuctionListState, AuctionState,
    ContainerSlot, ContainerState, ItemTemplateView, ScriptValue, UiScript, BIDDER, LIST, OWNER,
};

const LINEN: u32 = 2589;

/// Errors from the auction window's own code; the production load carries unrelated gaps.
fn auction_errors(errors: impl IntoIterator<Item = String>) -> Vec<String> {
    errors
        .into_iter()
        .filter(|e| e.contains("Auction"))
        .collect()
}

/// The Browse tab's class tree, built from the player's own DBCs as the app builds it.
fn tree() -> Vec<AuctionCategory> {
    let data = benilla_formats::wow_data().expect("install");
    let mut chain = benilla_formats::open_chain(&data).unwrap();
    let classes =
        crate::ui_items::ItemClasses(benilla_formats::load_item_classes(&mut chain).unwrap());
    let subclasses = crate::ui_items::ItemSubClasses(
        benilla_formats::load_item_sub_classes(&mut chain).unwrap(),
    );
    crate::ui_auction::categories(Some(&classes), Some(&subclasses))
}

fn row(
    auction_id: u32,
    name: &str,
    min_bid: u32,
    buyout: u32,
    bid: u32,
    owner: &str,
) -> AuctionItemRow {
    AuctionItemRow {
        auction_id,
        item_id: LINEN,
        name: Some(name.to_string()),
        texture: Some("Interface\\Icons\\INV_Fabric_Linen_01".into()),
        count: 5,
        quality: Some(1),
        level: 1,
        min_bid,
        min_increment: if bid > 0 { 100 } else { 0 },
        buyout_price: buyout,
        bid_amount: bid,
        high_bidder: AuctionHighBidder::Nil,
        owner: Some(owner.to_string()),
        owner_guid: 0x55,
        expires_at: 40_000.0,
        link: Some(format!("|cffffffff|Hitem:{LINEN}:0:0:0|h[{name}]|h|r")),
        ..Default::default()
    }
}

fn session(browse: Vec<AuctionItemRow>, owner: Vec<AuctionItemRow>) -> AuctionState {
    let mut lists: [AuctionListState; 3] = Default::default();
    lists[LIST] = AuctionListState {
        total: browse.len() as u32,
        rows: browse,
    };
    lists[OWNER] = AuctionListState {
        total: owner.len() as u32,
        rows: owner,
    };
    lists[BIDDER] = AuctionListState::default();
    AuctionState {
        lists,
        deposit_percent: 5,
    }
}

/// The production load with the class tree seated, then an open session on `state`.
fn open(state: AuctionState) -> UiScript {
    let (mut s, _failures) = super::layer_tests::production_load_with("auction", false, "", |_| {});
    let _ = s.take_errors();
    // `UnitName` and `UnitRace("player")` read the character record; the dress-up pane needs it.
    s.set_player_record(benilla_ui::script::PlayerRecord {
        name: "Probesix".into(),
        race: Some(("Dwarf".into(), "Dwarf".into())),
        class: Some(("Warrior".into(), "WARRIOR".into())),
        sex: 2,
    });
    s.set_auction_item_classes(tree());
    s.set_money(500_000);
    s.set_auction(Some(state));
    s.set_auction_can_query([true; 3], true);
    s.fire_event("AUCTION_HOUSE_SHOW", Vec::<ScriptValue>::new());
    s.tick(0.0);
    s
}

fn lock() -> std::sync::MutexGuard<'static, ()> {
    crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The issue's open state: the window shows the twelve classes, no rows before a search, the
/// search prompt and the portrait, and asks the owner and bidder lists.
#[test]
fn the_window_opens_on_the_twelve_classes_and_no_rows() {
    benilla_formats::wow_data_or_skip!();
    let _l = lock();
    let mut s = open(session(Vec::new(), Vec::new()));
    let errors = auction_errors(s.take_errors());
    assert!(errors.is_empty(), "auction window errors: {errors:#?}");
    s.run(
        r#"
        assert(IsAddOnLoaded("Blizzard_AuctionUI") == 1, "the addon loaded")
        assert(AuctionFrame:IsShown(), "the window is open")
        assert(AuctionFrameBrowse:IsShown(), "on the Browse tab")
        assert(select('#', GetAuctionItemClasses()) == 12, "twelve classes")
        assert(#CLASS_FILTERS == 12, "the category list holds them")
        for i = 1, 12 do
            local b = getglobal("AuctionFilterButton" .. i)
            assert(b:IsShown() and b:GetText() == CLASS_FILTERS[i], "class button " .. i)
        end
        assert(not AuctionFilterButton13:IsShown(), "no thirteenth class")
        assert(not BrowseButton1:IsShown(), "no rows before a search")
        assert(BrowseNoResultsText:GetText() == BROWSE_SEARCH_TEXT, "the search prompt")
        assert(GetAuctionSort("list", 1) == "quality", "the stock default sort")
        "#,
    )
    .unwrap();
    assert!(s.take_auction_owner_query(), "the open asks the owner list");
    assert_eq!(
        s.take_auction_bidder_query(),
        Some(0),
        "and the bidder list"
    );
}

/// A landed page paints its rows with their own prices, and a header click re-asks the server
/// with the new criteria in front.
#[test]
fn the_browse_list_paints_rows_and_a_header_click_asks_the_new_sort() {
    benilla_formats::wow_data_or_skip!();
    let _l = lock();
    let mut s = open(session(
        vec![
            row(1, "Linen Cloth", 1_000, 5_000, 0, "Seller"),
            row(2, "Linen Cloth", 1_500, 0, 2_000, "Seller"),
        ],
        Vec::new(),
    ));
    s.fire_event("AUCTION_ITEM_LIST_UPDATE", Vec::<ScriptValue>::new());
    let errors = auction_errors(s.take_errors());
    assert!(errors.is_empty(), "browse errors: {errors:#?}");
    let (first, second, third): (bool, bool, bool) = s
        .eval("return BrowseButton1:IsShown() ~= nil, BrowseButton2:IsShown() ~= nil, BrowseButton3:IsShown() ~= nil")
        .unwrap();
    assert_eq!(
        (first, second, third),
        (true, true, false),
        "two rows, the rest hidden"
    );
    let name: String = s.eval("return BrowseButton1Name:GetText()").unwrap();
    assert_eq!(name, "Linen Cloth");
    // The money frames carry the rows' prices, never the purse.
    let shown: (i64, i64) = s
        .eval("return BrowseButton1MoneyFrame.staticMoney, BrowseButton2MoneyFrame.staticMoney")
        .unwrap();
    assert!(
        [1_000, 2_000].contains(&shown.0) && [1_000, 2_000].contains(&shown.1),
        "each row shows its bid or opening price: {shown:?}"
    );

    let _ = s.take_auction_query();
    s.run("AuctionFrame_OnClickSortColumn('list', 'level')")
        .unwrap();
    let q = s
        .take_auction_query()
        .expect("the header click searches again");
    assert_eq!(
        q.sort.first(),
        Some(&(0, true)),
        "level leads, reversed: {:?}",
        q.sort
    );
    let errors = auction_errors(s.take_errors());
    assert!(errors.is_empty(), "sort errors: {errors:#?}");
}

/// Weapon opens its subclasses with no slot rows; Armor's Cloth opens the slot rows, the cloak
/// among them, and a search sends class 4 subclass 1.
#[test]
fn the_filter_tree_opens_as_the_reference() {
    benilla_formats::wow_data_or_skip!();
    let _l = lock();
    let mut s = open(session(Vec::new(), Vec::new()));
    let (subs, weapon_slots, cloth_slots, cloak): (i64, i64, i64, bool) = s
        .eval(
            r#"
            local function count(kind, shown) local n = 0
                for _, f in ipairs(OPEN_FILTER_LIST) do
                    if f[2] == kind and (not shown or f[6]) then n = n + 1 end
                end
                return n
            end
            AuctionFilterButton1:Click()
            local subs = count("subclass")
            AuctionFilterButton2:Click()
            local weapon = count("invtype", true)
            AuctionFilterButton1:Click()
            AuctionFilterButton2:Click()
            AuctionFilterButton4:Click()
            local cloth, cloak = 0, false
            for _, f in ipairs(OPEN_FILTER_LIST) do
                if f[2] == "invtype" and f[6] then
                    cloth = cloth + 1
                    if string.find(f[1], INVTYPE_CLOAK, 1, true) then cloak = true end
                end
            end
            return subs, weapon, cloth, cloak
            "#,
        )
        .unwrap();
    assert!(subs > 10, "the weapon subclasses: {subs}");
    assert_eq!(weapon_slots, 0, "weapons offer no slot rows");
    assert!(
        cloth_slots > 0 && cloak,
        "Cloth offers slots, the cloak too: {cloth_slots}"
    );
    let _ = s.take_auction_query();
    s.run("AuctionFrameBrowse_Search()").unwrap();
    let q = s.take_auction_query().expect("searched");
    assert_eq!((q.class, q.sub_class), (4, 1), "Armor, Cloth");
    let errors = auction_errors(s.take_errors());
    assert!(errors.is_empty(), "filter errors: {errors:#?}");
}

/// A bag of Linen: two full stacks and a partial one, a template that stacks to 20.
fn seat_linen(s: &mut UiScript) {
    s.set_item_template(
        LINEN,
        ItemTemplateView {
            name: "Linen Cloth".into(),
            quality: 1,
            stackable: 20,
            sell_price: 13,
            ..Default::default()
        },
    );
    let mut slots = HashMap::new();
    for (slot, count) in [(1u32, 20u32), (2, 20), (3, 5)] {
        slots.insert(
            slot,
            ContainerSlot {
                count,
                item_id: LINEN,
                link: Some(format!("|cffffffff|Hitem:{LINEN}:0:0:0|h[Linen Cloth]|h|r")),
                ..Default::default()
            },
        );
    }
    s.set_container(
        0,
        Some(ContainerState {
            bag_family: 0,
            name: None,
            num_slots: 16,
            slots,
        }),
    );
}

/// A right-click on the Auctions tab fills the sell slot; two stacks of 20 go to the app as a
/// multi-sell over the gathered stacks, and the progress bar follows its events.
#[test]
fn a_two_stack_sale_runs_as_a_multisell() {
    benilla_formats::wow_data_or_skip!();
    let _l = lock();
    let mut s = open(session(Vec::new(), Vec::new()));
    seat_linen(&mut s);
    s.run("AuctionFrameTab_OnClick(AuctionFrameTab3) UseContainerItem(0, 1)")
        .unwrap();
    s.fire_event("NEW_AUCTION_UPDATE", Vec::<ScriptValue>::new());
    let (name, stack, total, stack_entry): (String, i64, i64, bool) = s
        .eval(
            "local n, _, _, _, _, _, _, st, t = GetAuctionSellItemInfo() \
             return n, st, t, AuctionsStackSizeEntry:IsShown() ~= nil",
        )
        .unwrap();
    assert_eq!((name.as_str(), stack, total), ("Linen Cloth", 20, 45));
    assert!(stack_entry, "a stackable item shows the stack entries");
    let deposit: i64 = s.eval("return CalculateAuctionDeposit(1, 40)").unwrap();
    assert_eq!(deposit, 100, "the minimum deposit");

    s.run(
        "AuctionsStackSizeEntry:SetNumber(20) AuctionsNumStacksEntry:SetNumber(2) \
         MoneyInputFrame_SetCopper(StartPrice, 5000) MoneyInputFrame_SetCopper(BuyoutPrice, 10000) \
         AuctionFrameAuctions.duration = 1 AuctionsFrameAuctions_ValidateAuction()",
    )
    .unwrap();
    assert!(
        s.eval::<bool>("return AuctionsCreateAuctionButton:IsEnabled() == 1")
            .unwrap(),
        "two stacks of the 45 held validate"
    );
    s.run("AuctionsCreateAuctionButton_OnClick()").unwrap();
    let req = s.take_auction_start().expect("StartAuction queued");
    assert_eq!(
        (
            req.min_bid,
            req.buyout,
            req.run_time_minutes,
            req.stack_size,
            req.num_stacks
        ),
        (5_000, 10_000, 720, 20, 2)
    );
    assert!(!req.direct, "more than the sell item: a multi-sell");
    assert_eq!(req.items, vec![(0, 1, 20), (0, 2, 20)], "gathered until 40");

    // The stock handler reads the global `arg1`, which 12340 never sets, and its
    // `SetMinMaxValues` (`0x9713c0`) refuses the nil: the form blocks and the bar stays hidden.
    s.fire_event("AUCTION_MULTISELL_START", vec![ScriptValue::Int(2)]);
    let errors = auction_errors(s.take_errors());
    assert!(
        errors.len() == 1 && errors[0].contains("SetMinMaxValues"),
        "the reference's own error: {errors:#?}"
    );
    assert!(s
        .eval::<bool>("return AuctionsBlockFrame:IsShown() == 1")
        .unwrap());
    assert!(!s
        .eval::<bool>("return AuctionProgressFrame:IsShown() == 1")
        .unwrap());
    // A failure takes no argument and unblocks the form.
    s.fire_event("AUCTION_MULTISELL_FAILURE", Vec::<ScriptValue>::new());
    assert!(!s
        .eval::<bool>("return AuctionsBlockFrame:IsShown() == 1")
        .unwrap());
    let errors = auction_errors(s.take_errors());
    assert!(errors.is_empty(), "sell errors: {errors:#?}");
}

/// A sold row paints as sold and cannot be cancelled; a live one can.
#[test]
fn the_auctions_tab_paints_sold_rows_and_gates_the_cancel() {
    benilla_formats::wow_data_or_skip!();
    let _l = lock();
    let mut live = row(7, "Linen Cloth", 1_000, 5_000, 2_500, "Probesix");
    live.high_bidder = AuctionHighBidder::Name("Bidder".into());
    let mut sold = row(8, "Wool Cloth", 0, 4_000, 4_000, "Probesix");
    sold.sold = true;
    sold.high_bidder = AuctionHighBidder::Name("Bidder".into());
    let mut s = open(session(Vec::new(), vec![live, sold]));
    s.run("AuctionFrameTab_OnClick(AuctionFrameTab3)").unwrap();
    s.tick(0.0);
    s.fire_event("AUCTION_OWNED_LIST_UPDATE", Vec::<ScriptValue>::new());
    // The owner list's duration sort puts the sold row first.
    assert_eq!(s.auction_ids(OWNER), vec![8, 7]);
    let (sold_name, live_name, bidder): (String, String, String) = s
        .eval(
            "return AuctionsButton1Name:GetText(), AuctionsButton2Name:GetText(), \
             AuctionsButton2HighBidder:GetText()",
        )
        .unwrap();
    assert_eq!(live_name, "Linen Cloth");
    assert!(
        s.eval::<bool>(&format!(
            "return {sold_name:?} == format(AUCTION_ITEM_SOLD, 'Wool Cloth')"
        ))
        .unwrap(),
        "the sold row reads as sold: {sold_name:?}"
    );
    assert_eq!(bidder, "Bidder");
    s.run("AuctionsButton_OnClick(AuctionsButton1)").unwrap();
    assert!(!s
        .eval::<bool>("return AuctionsCancelAuctionButton:IsEnabled() == 1")
        .unwrap());
    s.run("AuctionsButton_OnClick(AuctionsButton2)").unwrap();
    assert!(s
        .eval::<bool>("return AuctionsCancelAuctionButton:IsEnabled() == 1")
        .unwrap());
    s.run("CancelAuction(GetSelectedAuctionItem('owner'))")
        .unwrap();
    assert_eq!(
        s.take_auction_cancels(),
        vec![7],
        "the cancel addresses the auction id"
    );
    let errors = auction_errors(s.take_errors());
    assert!(errors.is_empty(), "owner errors: {errors:#?}");
}

/// The bid and buyout buttons read the purse and the seller; a bid goes out by auction id.
#[test]
fn the_bid_gates_read_the_purse_and_the_seller() {
    benilla_formats::wow_data_or_skip!();
    let _l = lock();
    let mut s = open(session(
        vec![
            row(1, "Linen Cloth", 1_000, 5_000, 0, "Seller"),
            row(2, "Linen Cloth", 1_000, 5_000, 0, "Probesix"),
        ],
        Vec::new(),
    ));
    s.fire_event("AUCTION_ITEM_LIST_UPDATE", Vec::<ScriptValue>::new());
    // Equal rows keep the pushed order.
    assert_eq!(s.auction_ids(LIST), vec![1, 2]);
    s.run("BrowseButton_OnClick(BrowseButton1)").unwrap();
    assert!(s
        .eval::<bool>("return BrowseBidButton:IsEnabled() == 1")
        .unwrap());
    assert!(s
        .eval::<bool>("return BrowseBuyoutButton:IsEnabled() == 1")
        .unwrap());
    s.run("BrowseButton_OnClick(BrowseButton2)").unwrap();
    assert!(
        !s.eval::<bool>("return BrowseBidButton:IsEnabled() == 1")
            .unwrap(),
        "not on one's own auction"
    );
    s.run("BrowseButton_OnClick(BrowseButton1) PlaceAuctionBid('list', 1, 5000)")
        .unwrap();
    let bids = s.take_auction_bids();
    assert_eq!(
        (bids[0].auction_id, bids[0].amount),
        (1, 5_000),
        "a buyout by id"
    );
    let errors = auction_errors(s.take_errors());
    assert!(errors.is_empty(), "bid errors: {errors:#?}");
}

/// Closing the window closes the session client-side.
#[test]
fn hiding_the_window_closes_the_session() {
    benilla_formats::wow_data_or_skip!();
    let _l = lock();
    let mut s = open(session(Vec::new(), Vec::new()));
    let _ = s.take_auction_close();
    s.run("HideUIPanel(AuctionFrame)").unwrap();
    assert!(
        s.take_auction_close(),
        "AuctionFrame's OnHide calls CloseAuctionHouse"
    );
}

/// The tree off the install: twelve classes, all named, Glyph fifth; Armor's first five
/// subclasses offer the slot rows.
#[test]
fn the_class_tree_follows_the_twelve_row_table() {
    benilla_formats::wow_data_or_skip!();
    let tree = tree();
    assert_eq!(tree.len(), 12);
    assert!(tree.iter().all(|c| !c.name.is_empty()), "every class named");
    assert_eq!(tree[4].class_id, 16, "Glyph");
    assert!(!tree[11].has_subclass_filter, "Quest");
    let armor = &tree[1];
    let offering: Vec<u32> = armor
        .subclasses
        .iter()
        .filter(|s| s.has_inv_types)
        .map(|s| s.sub_id)
        .collect();
    assert_eq!(
        offering,
        vec![0, 1, 2, 3, 4],
        "not Shield, Libram, Idol, Totem, Sigil"
    );
}
