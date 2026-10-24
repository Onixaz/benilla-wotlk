//! The stock 3.3.5a guild bank window (`Blizzard_GuildBankUI`) on the production load, over the
//! state the app pushes for a guild bank with two tabs.

use benilla_ui::script::{
    GuildBankLogView, GuildBankRequest, GuildBankSlotView, GuildBankTabView, GuildBankView,
    GuildRankInfo, GuildState, GUILD_BANK_MONEY_LOG, GUILD_BANK_TABS, GUILD_BANK_TAB_SLOTS,
};

use super::bank_ui_tests::{bank_errors, container, load};

fn slot_view(item_id: u32, count: u32) -> Option<GuildBankSlotView> {
    Some(GuildBankSlotView {
        item_id,
        texture: Some(format!("Interface\\Icons\\INV_Test_{item_id}")),
        count,
        link: Some(format!(
            "|cffffffff|Hitem:{item_id}:0:0:0|h[Item {item_id}]|h|r"
        )),
        max_stack: 20,
        name: Some(format!("Item {item_id}")),
        ..Default::default()
    })
}

fn guild_bank() -> GuildBankView {
    let mut slots = vec![Vec::new(); GUILD_BANK_TABS];
    slots[0] = vec![None; GUILD_BANK_TAB_SLOTS];
    slots[0][0] = slot_view(2589, 20);
    slots[0][13] = slot_view(2592, 3);
    let mut logs = vec![Vec::new(); GUILD_BANK_TABS + 1];
    logs[0] = vec![GuildBankLogView {
        kind: 1,
        player: Some("Claude".into()),
        link: "|cffffffff|Hitem:2589:0:0:0|h[Linen Cloth]|h|r".into(),
        count: 20,
        written_at: 0,
        ..Default::default()
    }];
    logs[GUILD_BANK_MONEY_LOG] = vec![GuildBankLogView {
        kind: 4,
        player: Some("Claude".into()),
        money: 12_345,
        ..Default::default()
    }];
    GuildBankView {
        open: true,
        num_tabs: 2,
        tabs: (0..GUILD_BANK_TABS)
            .map(|i| GuildBankTabView {
                name: if i == 0 { "Mats".into() } else { String::new() },
                icon: if i == 0 {
                    "INV_Fabric_Linen_01".into()
                } else {
                    String::new()
                },
                text: String::new(),
            })
            .collect(),
        slots,
        withdrawals_remaining: 10,
        logs,
    }
}

/// A guild leader with every tab right, two hundred gold a day, and a tabard.
fn guild() -> GuildState {
    GuildState {
        in_guild: true,
        name: "Benilla".into(),
        rank_index: 0,
        is_leader: true,
        rights: 0x000f_f1ff,
        ranks: vec![GuildRankInfo {
            name: "Guild Master".into(),
            rights: 0x000f_f1ff,
            bank_money_per_day: 2_000_000,
            bank_tabs: vec![(0x7, 100); 6],
        }],
        bank_money: 50_000,
        bank_withdraw_money: -1,
        tabard: Some([1, 2, 3, 4, 5]),
        ..Default::default()
    }
}

/// Talking to a banker opens the stock window on its tabs, items and money; bank money dropped on
/// a bag slot withdraws through `ContainerFrameItemButton_OnClick`'s `"guildbankmoney"` arm with
/// no script error; the log tab asks for and shows the log.
#[test]
fn the_stock_guild_bank_opens_and_withdraws_money_onto_a_bag_slot() {
    benilla_formats::wow_data_or_skip!();
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut s = load("guildbank");
    s.set_container(0, container(16, &[]));
    s.set_guild(guild());
    s.set_guild_bank(guild_bank());
    s.fire_event("GUILDBANKFRAME_OPENED", vec![]);
    s.fire_event("GUILDBANK_UPDATE_TABS", vec![]);
    s.fire_event("GUILDBANKBAGSLOTS_CHANGED", vec![]);
    let errors = bank_errors(s.take_errors());
    assert!(errors.is_empty(), "guild bank errors: {errors:#?}");
    s.run(
        r#"
        assert(IsAddOnLoaded("Blizzard_GuildBankUI") == 1, "the addon loaded")
        assert(GuildBankFrame:IsShown(), "the window opened")
        assert(GuildBankTab1:IsShown() and GuildBankTab2:IsShown(), "both bought tabs show")
        local tex = GuildBankColumn1Button1IconTexture:GetTexture()
        assert(tex and tex:find("INV_Test_2589"), "slot 1 paints its item, got " .. tostring(tex))
        "#,
    )
    .unwrap();
    let requests = s.take_guild_bank_requests();
    assert!(
        requests.contains(&GuildBankRequest::QueryTab(0)),
        "the window asks for its current tab: {requests:?}"
    );

    // Bank money onto a backpack slot: the stock click withdraws it.
    s.run("OpenBag(0)").unwrap();
    s.run(
        r#"
        PickupGuildBankMoney(100)
        local kind, money = GetCursorInfo()
        assert(kind == "guildbankmoney" and money == 100, "mode 12 holds the coins")
        local button
        for i = 1, NUM_CONTAINER_FRAMES do
            local f = _G["ContainerFrame" .. i]
            if f:IsShown() and f:GetID() == 0 then button = _G["ContainerFrame" .. i .. "Item1"] end
        end
        assert(button, "the backpack is open")
        ContainerFrameItemButton_OnClick(button, "LeftButton")
        assert(GetCursorInfo() == nil, "the click cleared the cursor")
        "#,
    )
    .unwrap();
    let errors = bank_errors(s.take_errors());
    assert!(errors.is_empty(), "withdraw errors: {errors:#?}");
    assert_eq!(
        s.take_guild_bank_requests(),
        vec![GuildBankRequest::WithdrawMoney(100)]
    );

    // An item taken up locks its slot; a drop on a backpack slot withdraws it.
    s.run(
        r#"
        GuildBankColumn1Button1:Click("LeftButton")
        local _, _, locked = GetGuildBankItemInfo(1, 1)
        assert(locked == 1, "the taken slot is locked")
        PickupContainerItem(0, 1)
        "#,
    )
    .unwrap();
    let requests = s.take_guild_bank_requests();
    assert!(
        matches!(
            requests.as_slice(),
            [GuildBankRequest::WithdrawItem {
                src_tab: 0,
                src_slot: 0,
                item_id: 2589,
                bag: 0,
                bag_slot: 1,
                split: None
            }]
        ),
        "{requests:?}"
    );

    // The log tab asks for the log, and the answer prints a line.
    s.run("GuildBankFrameTab2:Click()").unwrap();
    let requests = s.take_guild_bank_requests();
    assert!(
        requests.contains(&GuildBankRequest::QueryLog(0)),
        "the log tab asks: {requests:?}"
    );
    s.fire_event("GUILDBANKLOG_UPDATE", vec![]);
    let errors = bank_errors(s.take_errors());
    assert!(errors.is_empty(), "log errors: {errors:#?}");
    assert!(
        s.eval::<i64>("return GuildBankMessageFrame:GetNumMessages()")
            .unwrap()
            >= 1,
        "the log line printed"
    );
    // The money log tab asks for the money log and prints its row through
    // `GetDenominationsFromCopper`.
    s.run("GuildBankFrameTab3:Click()").unwrap();
    let requests = s.take_guild_bank_requests();
    assert!(
        requests.contains(&GuildBankRequest::QueryLog(6)),
        "the money log tab asks: {requests:?}"
    );
    s.fire_event("GUILDBANKLOG_UPDATE", vec![]);
    let errors = bank_errors(s.take_errors());
    assert!(errors.is_empty(), "money log errors: {errors:#?}");
    assert!(
        s.eval::<i64>("return GuildBankMessageFrame:GetNumMessages()")
            .unwrap()
            >= 1,
        "the money log line printed"
    );
    s.fire_event("GUILDBANKFRAME_CLOSED", vec![]);
    assert!(!s.eval::<bool>("return GuildBankFrame:IsShown()").unwrap());
}
