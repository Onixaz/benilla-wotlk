//! The stock 3.3.5a mail window (`MailFrame.xml`) and the minimap mail icon on the production
//! load, over an inbox as the app resolves it from a 12340 list: a player letter with two
//! attachments, an auction house's sale invoice, and the minimap's three senders.

use benilla_ui::script::{
    ContainerSlot, ContainerState, ItemTemplateView, MailAttachmentView, MailInboxRow, MailInvoice,
    MailState, StationeryView, UiScript, MAIL_ATTACHMENT_SLOTS,
};

/// A stand-in font engine: each byte 7 wide, each line 14 tall, and an edit box's advances
/// answered inline as the live engine does, so a caret event precedes the text event.
struct Glyphs;

impl benilla_ui::script::TextMeasure for Glyphs {
    fn measure(&mut self, req: &benilla_ui::script::MeasureRequest) -> (f32, f32, f32) {
        let natural = req.text.split('\n').map(str::len).max().unwrap_or(0) as f32 * 7.0;
        (natural, req.text.split('\n').count() as f32 * 14.0, natural)
    }

    fn editbox_advances(
        &mut self,
        req: &benilla_ui::script::EditBoxAdvanceRequest,
    ) -> Option<(Vec<f32>, Vec<usize>, f32)> {
        let cum = (0..=req.text.len()).map(|i| i as f32 * 7.0).collect();
        if req.wrap_width.is_none() {
            return Some((cum, vec![0], 0.0));
        }
        let breaks = req.text.match_indices('\n').map(|(i, _)| i + 1);
        Some((cum, std::iter::once(0).chain(breaks).collect(), 14.0))
    }
}

/// One app frame: the layout resolve, then the tick.
fn frame(s: &mut UiScript) {
    s.resolve();
    s.tick(0.0);
}

/// Errors from the mail window's and the minimap icon's own code; the production load carries
/// unrelated 3.3.5a gaps.
fn mail_errors(errors: impl IntoIterator<Item = String>) -> Vec<String> {
    errors
        .into_iter()
        .filter(|e| {
            ["Mail", "Inbox", "Stationery", "Minimap"]
                .iter()
                .any(|k| e.contains(k))
        })
        .collect()
}

fn linen(slot_guid: u32, count: u32) -> MailAttachmentView {
    MailAttachmentView {
        item_id: 2589,
        item_guid_low: slot_guid,
        name: Some("Linen Cloth".into()),
        texture: Some("Interface\\Icons\\INV_Fabric_Linen_01".into()),
        count,
        quality: 1,
        link: Some("|cffffffff|Hitem:2589:0:0:0|h[Linen Cloth]|h|r".into()),
        random_property_id: 0,
    }
}

fn letter() -> MailInboxRow {
    let mut attachments = vec![None; MAIL_ATTACHMENT_SLOTS];
    attachments[0] = Some(linen(0x31, 5));
    attachments[3] = Some(linen(0x32, 7));
    MailInboxRow {
        message_id: 11,
        message_type: 0,
        flags: 0x10,
        money: 1234,
        days_left: 29.5,
        has_letter: true,
        package_icon: Some("Interface\\Icons\\INV_Crate_02".into()),
        stationery_icon: Some("Interface\\Icons\\INV_Misc_Note_01".into()),
        sender: Some("Claudetwo".into()),
        subject: "probe stacks".into(),
        attachments,
        body: Some("from Claudetwo".into()),
        stationery_texture: Some("STATIONERYTEST".into()),
        takeable: true,
        can_complain: true,
        ..Default::default()
    }
}

fn invoice() -> MailInboxRow {
    MailInboxRow {
        message_id: 12,
        message_type: 2,
        flags: 0x4,
        days_left: 29.0,
        has_letter: true,
        stationery_icon: Some("Interface\\Icons\\INV_Misc_Note_01".into()),
        sender: Some("Stormwind Auction House".into()),
        subject: "Sale Pending: Linen Cloth".into(),
        attachments: vec![None; MAIL_ATTACHMENT_SLOTS],
        stationery_texture: Some("STATIONERYTEST".into()),
        takeable: true,
        is_invoice: true,
        invoice: Some(MailInvoice {
            kind: "seller_temp_invoice",
            item_name: "Linen Cloth".into(),
            player_name: "Claude".into(),
            bid: 10_000,
            buyout: 10_000,
            deposit: 25,
            consignment: 500,
            money_delay: 3600,
            eta_hour: 14,
            eta_minute: 35,
        }),
        ..Default::default()
    }
}

fn seat_send_tab(s: &mut UiScript) {
    s.set_mail_stationeries(vec![StationeryView {
        id: 41,
        name: "Default Stationery".into(),
        icon: "Interface\\Icons\\INV_Misc_Note_01".into(),
        cost: Some(0),
        texture: "STATIONERYTEST".into(),
    }]);
    s.set_item_template(
        2589,
        ItemTemplateView {
            name: "Linen Cloth".into(),
            quality: 1,
            ..Default::default()
        },
    );
    let mut slots = std::collections::HashMap::new();
    for slot in [1u32, 2] {
        slots.insert(
            slot,
            ContainerSlot {
                item_id: 2589,
                count: 10 * slot,
                quality: Some(1),
                texture: Some("Interface\\Icons\\INV_Fabric_Linen_01".into()),
                link: Some("|cffffffff|Hitem:2589:0:0:0|h[Linen Cloth]|h|r".into()),
                ..Default::default()
            },
        );
    }
    s.set_container(
        0,
        Some(ContainerState {
            name: Some("Backpack".into()),
            num_slots: 16,
            slots,
            bag_family: 0,
        }),
    );
}

/// `MAIL_SHOW` opens the stock window and its inbox paints the 12340 header: the row's sender,
/// subject and attachment count; a click opens the letter, which lays out its two attachment
/// buttons and the money, and an invoice row fills the pending-sale invoice.
#[test]
fn the_stock_mail_window_paints_a_12340_inbox() {
    benilla_formats::wow_data_or_skip!();
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (mut s, _load_failures) =
        super::layer_tests::production_load_with("mail", false, "", |_| {});
    let _ = s.take_errors();
    // The live client always measures text; the edit boxes' caret events wait on it.
    s.set_text_measurer(Box::new(Glyphs));
    seat_send_tab(&mut s);
    s.set_mail(MailState {
        inbox: vec![letter(), invoice()],
        total: 2,
    });
    s.open_mailbox();
    frame(&mut s);
    s.fire_event("MAIL_INBOX_UPDATE", vec![]);
    frame(&mut s);
    let errors = mail_errors(s.take_errors());
    assert!(errors.is_empty(), "mail window errors: {errors:#?}");
    s.run(
        r#"
        assert(MailFrame:IsShown(), "MAIL_SHOW opened the window")
        assert(MailItem1Sender:GetText() == "Claudetwo", "the sender paints")
        assert(MailItem1Subject:GetText() == "probe stacks", "the subject paints")
        assert(MailItem1Button.itemCount == 2, "two attachments")
        assert(MailItem2Sender:GetText() == "Stormwind Auction House", "the house sends invoices")
        assert(not MailItem3Button:IsShown(), "two rows only")
        "#,
    )
    .unwrap();
    // The inbox list was asked for once.
    assert!(s
        .take_mail_commands()
        .iter()
        .any(|c| matches!(c, benilla_ui::script::MailCommand::GetList)));

    // Open the letter: its two attachments lay out, with the money button.
    s.run("MailItem1Button:Click()").unwrap();
    frame(&mut s);
    let errors = mail_errors(s.take_errors());
    assert!(errors.is_empty(), "open letter errors: {errors:#?}");
    s.run(
        r#"
        assert(OpenMailFrame:IsShown(), "the letter opened")
        assert(OpenMailBodyText:GetText() == "from Claudetwo", "the inline body shows")
        assert(OpenMailAttachmentButton1:IsShown(), "slot 1 shows")
        assert(OpenMailAttachmentButton4:IsShown(), "slot 4 shows")
        assert(not OpenMailAttachmentButton2:IsShown(), "an empty slot hides")
        assert(OpenMailMoneyButton:IsShown(), "the money shows")
        "#,
    )
    .unwrap();
    assert!(s.take_mail_commands().iter().any(|c| matches!(
        c,
        benilla_ui::script::MailCommand::MarkRead { message_id: 11 }
    )));

    // The invoice: a pending sale's funds line.
    s.run("HideUIPanel(OpenMailFrame) MailItem2Button:Click()")
        .unwrap();
    frame(&mut s);
    let errors = mail_errors(s.take_errors());
    assert!(errors.is_empty(), "invoice errors: {errors:#?}");
    s.run(
        r#"
        assert(OpenMailInvoiceFrame:IsShown(), "the invoice shows")
        assert(OpenMailInvoiceNotYetSent:IsShown(), "a pending sale says not yet sent")
        assert(OpenMailInvoicePurchaser:GetText():find("Claude"), "the buyer is named")
        "#,
    )
    .unwrap();
}

/// The Send tab: a bag click attaches to the next free slot, the postage counts each item, and
/// the stock send queues one `CMSG_SEND_MAIL` with both attachments.
#[test]
fn the_stock_send_tab_attaches_by_bag_click_and_sends_both() {
    benilla_formats::wow_data_or_skip!();
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (mut s, _load_failures) =
        super::layer_tests::production_load_with("mail-send", false, "", |_| {});
    let _ = s.take_errors();
    // The live client always measures text; the edit boxes' caret events wait on it.
    s.set_text_measurer(Box::new(Glyphs));
    seat_send_tab(&mut s);
    s.set_money(100_000);
    s.open_mailbox();
    frame(&mut s);
    // The open's list query is answered, ending the command in flight.
    let _ = s.take_mail_commands();
    s.mail_command_done(false);
    s.run("MailFrameTab_OnClick(nil, 2) UseContainerItem(0, 1) UseContainerItem(0, 2)")
        .unwrap();
    frame(&mut s);
    let errors = mail_errors(s.take_errors());
    assert!(errors.is_empty(), "send tab errors: {errors:#?}");
    s.run(
        r#"
        assert(SendMailFrame:IsShown(), "the Send tab is up")
        assert(SendMailAttachment1:GetNormalTexture() ~= nil, "slot 1 painted")
        assert(GetSendMailPrice() == 60, "30 per attachment, got " .. GetSendMailPrice())
        assert(SendMailSubjectEditBox:GetText() == "Linen Cloth (10)", "the subject took the item")
        SendMailNameEditBox:SetText("Claude")
        SendMailFrame_SendMail()
        "#,
    )
    .unwrap();
    match s.take_mail_commands().as_slice() {
        [benilla_ui::script::MailCommand::Send(req)] => {
            assert_eq!(req.target, "Claude");
            assert_eq!(req.items, vec![(0, 0, 1), (1, 0, 2)]);
            assert_eq!(
                req.stationery, 41,
                "the reset selected the first stationery"
            );
        }
        other => panic!("one send, got {other:?}"),
    }
}

/// The minimap icon (#427): `UPDATE_PENDING_MAIL` with mail waiting shows it, and its tooltip
/// reads `HAVE_MAIL_FROM` and the delivered senders, one per line.
#[test]
fn the_minimap_mail_tooltip_lists_the_latest_senders() {
    benilla_formats::wow_data_or_skip!();
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (mut s, _load_failures) =
        super::layer_tests::production_load_with("mail-minimap", false, "", |_| {});
    let _ = s.take_errors();
    s.set_has_new_mail(true);
    s.set_mail_latest_senders(vec![
        Some("Claudetwo".into()),
        Some("Stormwind Auction House".into()),
    ]);
    s.fire_event("UPDATE_PENDING_MAIL", vec![]);
    frame(&mut s);
    s.run("MiniMapMailFrame:GetScript('OnEnter')(MiniMapMailFrame)")
        .unwrap();
    let errors = mail_errors(s.take_errors());
    assert!(errors.is_empty(), "minimap mail errors: {errors:#?}");
    assert!(s.eval::<bool>("return MiniMapMailFrame:IsShown()").unwrap());
    let tip: String = s.eval("return GameTooltipTextLeft1:GetText()").unwrap();
    let head: String = s.eval("return HAVE_MAIL_FROM").unwrap();
    assert_eq!(tip, format!("{head}\nClaudetwo\nStormwind Auction House"));

    // No delivered sender: the bare `HAVE_MAIL`.
    s.set_mail_latest_senders(vec![]);
    s.run("MiniMapMailFrame:GetScript('OnEnter')(MiniMapMailFrame)")
        .unwrap();
    let tip: String = s.eval("return GameTooltipTextLeft1:GetText()").unwrap();
    assert_eq!(tip, s.eval::<String>("return HAVE_MAIL").unwrap());

    // Mail read and the server says none: the icon hides.
    s.set_has_new_mail(false);
    s.fire_event("UPDATE_PENDING_MAIL", vec![]);
    frame(&mut s);
    assert!(!s.eval::<bool>("return MiniMapMailFrame:IsShown()").unwrap());
}
