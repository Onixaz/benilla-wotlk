use super::*;
use crate::script::{ContainerSlot, ContainerState, ItemTemplateView, UiScript};

/// A send needs a stationery: select the default, as the stock `SendMailFrame_Reset` does.
fn select_default_stationery(s: &mut UiScript) {
    s.set_mail_stationeries(vec![StationeryView {
        id: 41,
        name: "Default Stationery".into(),
        icon: "Interface\\Icons\\INV_Letter_15".into(),
        cost: Some(0),
        texture: "STATIONERYTEST".into(),
    }]);
    s.run("SelectStationery(1)").unwrap();
}

fn attachment(item_id: u32, guid: u32, count: u32) -> MailAttachmentView {
    MailAttachmentView {
        item_id,
        item_guid_low: guid,
        name: Some(format!("Item {item_id}")),
        texture: Some("Interface\\Icons\\INV_Fabric_Linen_01".into()),
        count,
        quality: 1,
        link: Some(format!(
            "|cffffffff|Hitem:{item_id}:0:0:0:0:0:0:0:0|h[Item {item_id}]|h|r"
        )),
        random_property_id: 0,
    }
}

/// A player's letter, with attachments in the given slots.
fn row(id: u32, slots: &[(usize, MailAttachmentView)]) -> MailInboxRow {
    let mut attachments = vec![None; MAIL_ATTACHMENT_SLOTS];
    for (slot, a) in slots {
        attachments[*slot] = Some(a.clone());
    }
    MailInboxRow {
        message_id: id,
        message_type: 0,
        flags: 0x10,
        money: 0,
        cod: 0,
        days_left: 29.5,
        is_gm: false,
        has_letter: true,
        delete_pending: false,
        package_icon: Some("Interface\\Icons\\INV_Misc_Bag_08".into()),
        stationery_icon: Some("Interface\\Icons\\INV_Letter_15".into()),
        sender: Some("Thrall".into()),
        subject: "Orders".into(),
        attachments,
        body: Some("Lok'tar.".into()),
        stationery_texture: Some("STATIONERYTEST".into()),
        takeable: true,
        is_invoice: false,
        invoice: None,
        can_complain: true,
    }
}

/// An open mailbox with the given rows.
fn opened(rows: Vec<MailInboxRow>) -> UiScript {
    let mut s = UiScript::new().unwrap();
    let total = rows.len() as u32;
    s.set_mail(MailState { inbox: rows, total });
    s.open_mailbox();
    s.tick(0.0);
    s
}

fn linen_bag() -> ContainerState {
    let mut slots = std::collections::HashMap::new();
    for slot in [1u32, 2, 3] {
        slots.insert(
            slot,
            ContainerSlot {
                item_id: 2589,
                count: 5 * slot,
                quality: Some(1),
                texture: Some("Interface\\Icons\\INV_Fabric_Linen_01".into()),
                link: Some("|cffffffff|Hitem:2589|h[Linen Cloth]|h|r".into()),
                ..Default::default()
            },
        );
    }
    ContainerState {
        name: Some("Backpack".into()),
        num_slots: 16,
        slots,
        bag_family: 0,
    }
}

fn with_linen(s: &mut UiScript) {
    s.set_item_template(
        2589,
        ItemTemplateView {
            name: "Linen Cloth".into(),
            quality: 1,
            ..Default::default()
        },
    );
    s.set_container(0, Some(linen_bag()));
}

#[test]
fn the_inbox_counts_shown_rows_and_the_server_total() {
    let mut s = UiScript::new().unwrap();
    s.set_mail(MailState {
        inbox: vec![row(1, &[])],
        total: 60,
    });
    assert_eq!(
        s.eval::<(i64, i64)>("return GetInboxNumItems()").unwrap(),
        (1, 60)
    );
}

#[test]
fn the_header_answers_fourteen_values_on_every_path() {
    let one = attachment(2589, 0x31, 5);
    let s = opened(vec![
        row(1, &[(3, one.clone())]),
        row(2, &[(0, one.clone()), (1, one)]),
    ]);
    assert_eq!(s.arity("GetInboxHeaderInfo(1)").unwrap(), 14);
    assert_eq!(s.arity("GetInboxHeaderInfo(9)").unwrap(), 14);
    assert!(s
        .eval::<bool>(
            "local a,b,c,d,money,cod,days,n,read,ret,tc,reply,gm,qty = GetInboxHeaderInfo(1)
             return c == 'Thrall' and d == 'Orders' and n == 1 and read == nil and reply == 1
                and gm == nil and qty == 5 and days == 29.5",
        )
        .unwrap());
    // Two attachments: a count of two and no single quantity.
    assert!(s
        .eval::<bool>(
            "local _,_,_,_,_,_,_,n,_,_,_,_,_,qty = GetInboxHeaderInfo(2) return n == 2 and qty == nil",
        )
        .unwrap());
    // Out of range: four nils, three zeros, seven nils.
    assert!(s
        .eval::<bool>(
            "local a,b,c,d,e,f,g,h = GetInboxHeaderInfo(9)
             return a == nil and d == nil and e == 0 and f == 0 and g == 0 and h == nil",
        )
        .unwrap());
    assert!(
        s.run("GetInboxHeaderInfo('x')").is_err(),
        "a non-number raises"
    );
}

#[test]
fn a_gm_or_non_player_mail_cannot_be_replied_to() {
    let mut gm = row(1, &[]);
    gm.is_gm = true;
    let mut ah = row(2, &[]);
    ah.message_type = 2;
    let s = opened(vec![gm, ah]);
    assert!(s
        .eval::<bool>(
            "local _,_,_,_,_,_,_,_,_,_,_,reply,gm = GetInboxHeaderInfo(1) return reply == nil and gm == 1",
        )
        .unwrap());
    assert!(s
        .eval::<bool>(
            "local _,_,_,_,_,_,_,_,_,_,_,reply,gm = GetInboxHeaderInfo(2) return reply == nil and gm == nil",
        )
        .unwrap());
}

#[test]
fn get_inbox_item_reads_a_slot_or_the_first_filled_one() {
    let mut pending = attachment(6948, 0x40, 1);
    pending.name = None;
    let s = opened(vec![row(
        1,
        &[(2, attachment(2589, 0x31, 5)), (4, pending)],
    )]);
    s.run("UNKNOWNOBJECT = 'Unknown'").unwrap();
    let (name, _tex, count, quality, can_use): (String, String, i64, i64, i64) =
        s.eval("return GetInboxItem(1)").unwrap();
    assert_eq!(
        (name.as_str(), count, quality, can_use),
        ("Item 2589", 5, 1, 1)
    );
    assert_eq!(
        s.eval::<String>("return (GetInboxItem(1, 3))").unwrap(),
        "Item 2589",
        "slot 3 is index 2"
    );
    assert!(s
        .eval::<bool>("local n, t, c, q, u = GetInboxItem(1, 1) return n == nil and c == 0 and q == 0 and u == nil")
        .unwrap());
    // A template in flight answers the unknown object.
    assert!(s
        .eval::<bool>(
            "local n, t, c = GetInboxItem(1, 5) return n == 'Unknown' and c == 0
             and t == 'Interface\\\\Icons\\\\INV_Misc_QuestionMark'",
        )
        .unwrap());
    assert_eq!(s.arity("GetInboxItemLink(1, 1)").unwrap(), 0);
    assert!(s
        .eval::<String>("return GetInboxItemLink(1, 3)")
        .unwrap()
        .contains("item:2589"));
}

#[test]
fn the_invoice_answers_ten_values_or_a_seven_value_miss() {
    let mut r = row(1, &[]);
    r.message_type = 2;
    r.is_invoice = true;
    r.invoice = Some(MailInvoice {
        kind: "seller",
        item_name: "Linen Cloth".into(),
        player_name: "Twowarrior".into(),
        bid: 100,
        buyout: 100,
        deposit: 5,
        consignment: 5,
        money_delay: 0,
        eta_hour: 0,
        eta_minute: 0,
    });
    let s = opened(vec![r, row(2, &[])]);
    assert_eq!(s.arity("GetInboxInvoiceInfo(1)").unwrap(), 10);
    assert_eq!(
        s.eval::<(String, String, String, i64)>(
            "local a,b,c,d = GetInboxInvoiceInfo(1) return a,b,c,d"
        )
        .unwrap(),
        (
            "seller".into(),
            "Linen Cloth".into(),
            "Twowarrior".into(),
            100
        )
    );
    assert_eq!(s.arity("GetInboxInvoiceInfo(2)").unwrap(), 7);
    assert!(s
        .eval::<bool>(
            "local a,b,c,d,e,f,g = GetInboxInvoiceInfo(2) return a == nil and c == nil and d == 0 and g == 0",
        )
        .unwrap());
}

#[test]
fn reading_a_letter_marks_it_read_once() {
    let mut s = opened(vec![row(7, &[])]);
    let (body, tex, takeable, invoice): (String, String, Value, Value) =
        s.eval("return GetInboxText(1)").unwrap();
    assert_eq!(
        (body.as_str(), tex.as_str()),
        ("Lok'tar.", "STATIONERYTEST")
    );
    assert_eq!(takeable, Value::Integer(1));
    assert!(matches!(invoice, Value::Nil));
    s.run("GetInboxText(1)").unwrap();
    assert_eq!(
        s.take_mail_commands(),
        vec![MailCommand::MarkRead { message_id: 7 }],
        "an already-read mail sends nothing"
    );
    assert_eq!(s.arity("GetInboxText(5)").unwrap(), 4);
}

#[test]
fn check_inbox_waits_for_a_mailbox_a_free_command_and_a_minute() {
    let mut s = UiScript::new().unwrap();
    s.run("CheckInbox()").unwrap();
    assert!(s.take_mail_commands().is_empty(), "no mailbox");
    s.open_mailbox();
    s.run("CheckInbox() CheckInbox()").unwrap();
    assert_eq!(
        s.take_mail_commands(),
        vec![MailCommand::GetList],
        "the second waits on the first's result"
    );
    s.mail_command_done(false);
    s.tick(0.0);
    s.run(
        r#"FIRED = 0
           local f = CreateFrame("Frame")
           f:RegisterEvent("MAIL_INBOX_UPDATE")
           f:SetScript("OnEvent", function() FIRED = FIRED + 1 end)"#,
    )
    .unwrap();
    s.run("CheckInbox()").unwrap();
    s.tick(0.0);
    assert!(
        s.take_mail_commands().is_empty(),
        "throttled to once a minute"
    );
    assert_eq!(
        s.eval::<i64>("return FIRED").unwrap(),
        1,
        "but the cached rows repaint"
    );
    s.tick(61.0);
    s.run("CheckInbox()").unwrap();
    assert_eq!(s.take_mail_commands(), vec![MailCommand::GetList]);
}

#[test]
fn takes_return_and_delete_follow_the_references_gates() {
    let mut money = row(1, &[]);
    money.money = 500;
    let item = row(2, &[(0, attachment(2589, 0x31, 5))]);
    let mut cod = row(3, &[(0, attachment(2589, 0x32, 5))]);
    cod.cod = 10_000;
    let plain = row(4, &[]);
    let mut s = opened(vec![money, item, cod, plain]);
    s.set_money(100);

    // A player's mail with money returns and does not delete.
    assert!(s
        .eval::<bool>("return InboxItemCanDelete(1) == nil")
        .unwrap());
    s.run("DeleteInboxItem(1)").unwrap();
    assert!(s.take_mail_commands().is_empty());
    s.run("ReturnInboxItem(1)").unwrap();
    assert_eq!(
        s.take_mail_commands(),
        vec![MailCommand::Return { message_id: 1 }]
    );
    s.run("TakeInboxMoney(1)").unwrap();
    assert!(s.take_mail_commands().is_empty(), "one command at a time");
    s.mail_command_done(false);

    // A COD the purse cannot pay is refused locally.
    s.run("TakeInboxItem(3, 1)").unwrap();
    assert!(s.take_mail_commands().is_empty());
    assert_eq!(s.take_ui_errors(), vec!["ERR_NOT_ENOUGH_MONEY"]);

    s.run("TakeInboxItem(2, 1)").unwrap();
    assert_eq!(
        s.take_mail_commands(),
        vec![MailCommand::TakeItem {
            message_id: 2,
            item_guid_low: 0x31,
            auto_loot: false
        }]
    );
    s.mail_command_done(false);

    // An empty player letter deletes.
    assert!(s.eval::<bool>("return InboxItemCanDelete(4) == 1").unwrap());
    s.run("DeleteInboxItem(4)").unwrap();
    assert_eq!(
        s.take_mail_commands(),
        vec![MailCommand::Delete { message_id: 4 }]
    );
    s.mail_command_done(false);
    s.run("DeleteInboxItem(4)").unwrap();
    assert!(
        s.take_mail_commands().is_empty(),
        "a delete already in flight"
    );
}

#[test]
fn auto_loot_takes_the_money_then_the_first_attachment() {
    let mut r = row(
        1,
        &[
            (1, attachment(2589, 0x31, 5)),
            (6, attachment(4306, 0x33, 2)),
        ],
    );
    r.money = 50;
    let mut s = opened(vec![r]);
    s.run("AutoLootMailItem(1)").unwrap();
    assert_eq!(
        s.take_mail_commands(),
        vec![
            MailCommand::TakeMoney { message_id: 1 },
            MailCommand::TakeItem {
                message_id: 1,
                item_guid_low: 0x31,
                auto_loot: true
            },
        ]
    );
}

#[test]
fn send_mail_needs_a_target_a_subject_a_stationery_and_no_money_with_cod() {
    let mut s = opened(vec![]);
    s.set_money(10_000);
    s.run("SendMail('Jaina', 'Hi', 'x')").unwrap();
    assert!(s.take_mail_commands().is_empty(), "no stationery");
    select_default_stationery(&mut s);
    s.run("SendMail('', 'Hi', 'x') SendMail('Jaina', '', 'x')")
        .unwrap();
    assert!(
        s.take_mail_commands().is_empty(),
        "an empty target or subject"
    );
    // COD with no attachment stores nothing.
    s.run("SetSendMailCOD(50)").unwrap();
    assert_eq!(s.eval::<i64>("return GetSendMailCOD()").unwrap(), 0);
    assert!(s
        .eval::<bool>("return SetSendMailMoney(1234) == 1")
        .unwrap());
    s.run("SendMail('Jaina', 'Hi', nil)").unwrap();
    match s.take_mail_commands().as_slice() {
        [MailCommand::Send(req)] => {
            assert_eq!((req.target.as_str(), req.subject.as_str()), ("Jaina", "Hi"));
            assert_eq!(
                (req.body.as_str(), req.money, req.stationery),
                ("", 1234, 41)
            );
            assert!(req.items.is_empty());
        }
        other => panic!("one send, got {other:?}"),
    }
    s.run("SendMail('Jaina', 'Hi', 'again')").unwrap();
    assert!(s.take_mail_commands().is_empty(), "a send in flight");
    assert!(
        s.run("SendMail(nil, 'Hi', 'x')").is_err(),
        "a non-string target raises"
    );
}

#[test]
fn attachments_fill_free_slots_lock_their_bag_slots_and_cost_postage() {
    let mut s = opened(vec![]);
    s.set_money(10_000);
    with_linen(&mut s);
    select_default_stationery(&mut s);
    // Without the Send tab a bag click is not an attach.
    s.run("UseContainerItem(0, 1)").unwrap();
    assert!(s.eval::<bool>("return GetSendMailItem(1) == nil").unwrap());
    s.run("SetSendMailShowing(true) UseContainerItem(0, 1) UseContainerItem(0, 2)")
        .unwrap();
    assert_eq!(
        s.eval::<(String, i64)>("local n, t, c = GetSendMailItem(2) return n, c")
            .unwrap(),
        ("Linen Cloth".into(), 10)
    );
    assert!(
        s.eval::<bool>("local _, _, locked = GetContainerItemInfo(0, 1) return locked")
            .unwrap(),
        "an attached slot reads locked"
    );
    assert_eq!(s.eval::<i64>("return GetSendMailPrice()").unwrap(), 60);
    // COD now stores, and money with COD sends nothing.
    s.run("SetSendMailCOD(50) SetSendMailMoney(5)").unwrap();
    s.run("SendMail('Jaina', 'Hi', '')").unwrap();
    assert!(s.take_mail_commands().is_empty());
    s.run("SetSendMailMoney(0) SendMail('Jaina', 'Hi', '')")
        .unwrap();
    match s.take_mail_commands().as_slice() {
        [MailCommand::Send(req)] => {
            assert_eq!(req.items, vec![(0, 0, 1), (1, 0, 2)]);
            assert_eq!(req.cod, 50);
        }
        other => panic!("one send, got {other:?}"),
    }
    assert!(
        s.run("SetSendMailShowing(1)").is_err(),
        "a non-boolean raises"
    );
}

#[test]
fn the_attachment_button_swaps_with_the_cursor_and_right_click_clears() {
    let mut s = opened(vec![]);
    with_linen(&mut s);
    s.run("PickupContainerItem(0, 1) ClickSendMailItemButton(3)")
        .unwrap();
    assert!(s.eval::<bool>("return not CursorHasItem()").unwrap());
    assert!(s.eval::<bool>("return GetSendMailItem(3) ~= nil").unwrap());
    // A second item on the same slot: it attaches and the first comes back to the cursor.
    s.run("PickupContainerItem(0, 2) ClickSendMailItemButton(3)")
        .unwrap();
    assert_eq!(
        s.eval::<i64>("local _, _, c = GetSendMailItem(3) return c")
            .unwrap(),
        10
    );
    assert!(s.eval::<bool>("return CursorHasItem()").unwrap());
    s.run("ClearCursor() ClickSendMailItemButton(3, true)")
        .unwrap();
    assert!(s.eval::<bool>("return GetSendMailItem(3) == nil").unwrap());
    assert!(
        !s.eval::<bool>("local _, _, locked = GetContainerItemInfo(0, 2) return locked")
            .unwrap(),
        "the cleared attachment unlocks"
    );
}

#[test]
fn opening_resets_the_form_before_mail_show_and_closing_ends_the_session() {
    let mut s = UiScript::new().unwrap();
    s.run(
        r#"FIRED = {}
           local f = CreateFrame("Frame")
           for _, e in ipairs({ "SEND_MAIL_MONEY_CHANGED", "SEND_MAIL_COD_CHANGED",
                                "MAIL_SEND_SUCCESS", "MAIL_SHOW", "MAIL_CLOSED" }) do
               f:RegisterEvent(e)
           end
           f:SetScript("OnEvent", function() table.insert(FIRED, event) end)"#,
    )
    .unwrap();
    s.open_mailbox();
    s.tick(0.0);
    assert_eq!(
        s.eval::<Vec<String>>("return FIRED").unwrap(),
        [
            "SEND_MAIL_MONEY_CHANGED",
            "SEND_MAIL_COD_CHANGED",
            "MAIL_SEND_SUCCESS",
            "MAIL_SHOW"
        ]
    );
    s.run("FIRED = {} CloseMail() CloseMail()").unwrap();
    s.tick(0.0);
    assert_eq!(
        s.eval::<Vec<String>>("return FIRED").unwrap(),
        ["MAIL_CLOSED"]
    );
    assert_eq!(s.take_mail_commands(), vec![MailCommand::Close]);
}

#[test]
fn the_latest_senders_push_nil_for_an_unresolved_name() {
    let mut s = UiScript::new().unwrap();
    assert_eq!(s.arity("GetLatestThreeSenders()").unwrap(), 0);
    s.set_mail_latest_senders(vec![Some("Thrall".into()), None]);
    assert_eq!(s.arity("GetLatestThreeSenders()").unwrap(), 2);
    assert!(s
        .eval::<bool>("local a, b = GetLatestThreeSenders() return a == 'Thrall' and b == nil")
        .unwrap());
}

#[test]
fn packages_and_stationery_answer_like_the_client() {
    let mut s = UiScript::new().unwrap();
    s.set_mail_packages(vec![PackageView {
        id: 2,
        name: "Test Package".into(),
        icon: "Interface\\Icons\\INV_BOX_04".into(),
        cost: 10,
    }]);
    assert_eq!(s.eval::<i64>("return GetNumPackages()").unwrap(), 1);
    assert!(s
        .eval::<bool>("local a, b, c = GetPackageInfo(2) return a == nil and b == nil and c == 0")
        .unwrap());
    select_default_stationery(&mut s);
    assert_eq!(
        s.eval::<String>("return GetSelectedStationeryTexture()")
            .unwrap(),
        "STATIONERYTEST"
    );
    s.run("SelectPackage(1)").unwrap();
    assert_eq!(s.eval::<i64>("return GetSendMailPrice()").unwrap(), 40);
    s.run("ClearSendMail()").unwrap();
    assert!(s
        .eval::<bool>("return GetSelectedStationeryTexture() == nil")
        .unwrap());
}
