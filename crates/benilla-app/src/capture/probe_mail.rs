//! The mail live probe (`WOW_PROBE_MAIL`), driving the stock 3.3.5a mail window through the live
//! Lua VM as clicks would. One `PROBE_MAIL:` line per step with PASS/FAIL/SKIP, then
//! `PROBE_MAIL: DONE pass=<n> fail=<m>`, and it exits. The switches are `docs/CONTRIBUTING.md`,
//! "Running it unattended". Three roles:
//!
//! - `1`: GM-mails the probe's own character a letter, money and two item stacks, opens the
//!   Goldshire mailbox and works the inbox: the header, the inline body, the letter copy and its
//!   reading, the money, one attachment then auto-loot, two refused sends, a delete, the close.
//! - `send:<name>`: at the mailbox, attaches two bag stacks on the Send tab and mails them to
//!   `<name>`, then mails it a letter with money.
//! - `receive[:<name>]`: reads everything waiting until the icon is dark, waits for a new arrival
//!   (`WOW_PROBE_MAIL_WAIT` seconds, default 240), checks the minimap tooltip's senders, then
//!   re-lists and reads what came, auction invoices included. Run it beside a `send` client, or
//!   beside `WOW_PROBE_AUCTION` buying this character's auctions.
//!
//! AzerothCore's `.send` GM commands mail with no delivery delay; a player's mail with items to
//! another account's character waits `MailDeliveryDelay` (an hour by default), money and text not.

use bevy::prelude::*;

use benilla_protocol::messages::mail_flags;
use benilla_protocol::EntityKind;
use benilla_ui::script::UiScript;

use super::probes::ProbeClock;
use crate::net::{ChatKind, ClientCommand, Guid, NetCommands, NetEntity, ObjectStore, SelfPlayer};
use crate::player::Player;
use crate::ui_mail::MailOpen;

/// The Goldshire mailbox (map 0).
const MAILBOX_AT: [f32; 3] = [-9455.99, 45.82, 56.44];
/// `GAMEOBJECT_TYPE_MAILBOX`.
const GO_TYPE_MAILBOX: i32 = 19;
/// Wider than the interaction gate, so a slightly-off `.go` landing still finds it.
const MAILBOX_SCAN_RANGE: f32 = 10.0;
/// The GM-mailed and Send-tab stacks: Linen Cloth and Silk Cloth.
const LINEN: u32 = 2589;
const SILK: u32 = 4306;
/// The GM-mailed money and the Send tab's, in copper.
const MONEY_COPPER: u32 = 1234;
const SEND_COPPER: u32 = 77;
/// A mail copy's item, AzerothCore's `MAIL_BODY_ITEM_TEMPLATE` (`Mail.h`).
const PLAIN_LETTER: u32 = 9311;

const SETTLE_SECS: f64 = 4.0;
const SCAN_TIMEOUT_SECS: f64 = 20.0;
const ACTION_TIMEOUT_SECS: f64 = 12.0;
/// `CheckInbox`'s throttle plus a margin: a re-list sooner answers the cached rows.
const RELIST_SECS: f64 = 62.0;
const DEFAULT_WAIT_SECS: f64 = 240.0;

/// The Lua hooks: event counts, and the message lines in order.
const HOOKS: &str = r#"
if not ProbeMail then
    ProbeMail = { ev = {}, msgs = {} }
    local f = CreateFrame("Frame")
    for _, e in ipairs({ "UI_ERROR_MESSAGE", "UI_INFO_MESSAGE", "MAIL_SHOW", "MAIL_CLOSED",
                         "MAIL_INBOX_UPDATE", "MAIL_SEND_SUCCESS", "MAIL_SUCCESS", "MAIL_FAILED",
                         "MAIL_SEND_INFO_UPDATE", "UPDATE_PENDING_MAIL", "CLOSE_INBOX_ITEM" }) do
        f:RegisterEvent(e)
    end
    f:SetScript("OnEvent", function(self, event, a1)
        ProbeMail.ev[event] = (ProbeMail.ev[event] or 0) + 1
        if event == "UI_ERROR_MESSAGE" or event == "UI_INFO_MESSAGE" then
            table.insert(ProbeMail.msgs, a1 or "")
        end
    end)
end
"#;

pub(crate) struct ProbeMailPlugin;

impl Plugin for ProbeMailPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MailProbe>()
            .add_systems(Update, mail_probe);
    }
}

#[derive(Default, Clone, PartialEq)]
enum Role {
    #[default]
    Solo,
    Send(String),
    Receive(Option<String>),
}

impl Role {
    fn from_env() -> Self {
        let v = std::env::var("WOW_PROBE_MAIL").unwrap_or_default();
        match v.split_once(':') {
            Some(("send", peer)) => Role::Send(peer.to_string()),
            Some(("receive", peer)) => Role::Receive(Some(peer.to_string())),
            _ if v == "receive" => Role::Receive(None),
            _ => Role::Solo,
        }
    }
}

#[derive(Default, Clone, Copy, PartialEq, Debug)]
enum Step {
    #[default]
    Init,
    FindMailbox,
    WaitList,
    // Solo.
    Header,
    ReadLetter,
    CopyLetter,
    ReadCopy,
    TakeMoney,
    TakeItem,
    AutoLoot,
    SendSelf,
    SendBad,
    Delete,
    Close,
    // Send.
    Attach,
    SendItems,
    SendMoney,
    // Receive.
    ReadAll,
    WaitDark,
    WaitArrival,
    WaitRelist,
    ReadNew,
    Done,
}

#[derive(Resource, Default)]
struct MailProbe {
    role: Role,
    me: String,
    step: Step,
    /// When the step began.
    since: f64,
    /// The step's action went out.
    sent: bool,
    /// A counter's value when the action went out.
    baseline: i64,
    /// The row a step works on, by message id.
    target: Option<u32>,
    first_list_at: Option<f64>,
    /// The message ids listed before the wait, so the re-list tells the new ones.
    seen: Vec<u32>,
    /// The last logged `HasNewMail()` and senders, for the change trace.
    traced: Option<(bool, String)>,
    /// When the loading screen last cleared; a world enter resets the mail session, so the arc
    /// starts only once the screen has been down a while.
    clear_since: Option<f64>,
    /// The mailbox in use, to re-open after the letter reader closed the window.
    mailbox: Option<u64>,
    passes: u32,
    fails: u32,
    exited: bool,
}

impl MailProbe {
    fn go(&mut self, step: Step, now: f64) {
        self.step = step;
        self.since = now;
        self.sent = false;
        self.baseline = 0;
    }

    fn pass(&mut self, what: &str) {
        info!("PROBE_MAIL: PASS {what}");
        self.passes += 1;
    }

    fn fail(&mut self, what: &str) {
        error!("PROBE_MAIL: FAIL {what}");
        self.fails += 1;
    }

    /// A verb step waits a beat after the previous result, so the window's command-pending flag
    /// has cleared, as a player's next click would find it.
    fn ready(&self, now: f64) -> bool {
        now - self.since >= 0.5
    }

    fn timed_out(&self, now: f64, secs: f64) -> bool {
        now - self.since > secs
    }
}

fn lua_i64(script: &UiScript, expr: &str) -> i64 {
    script.eval::<i64>(&format!("return {expr}")).unwrap_or(-1)
}

fn lua_bool(script: &UiScript, expr: &str) -> bool {
    script
        .eval::<bool>(&format!("return ({expr}) and true or false"))
        .unwrap_or(false)
}

fn lua_str(script: &UiScript, expr: &str) -> String {
    script
        .eval::<String>(&format!("return tostring({expr})"))
        .unwrap_or_default()
}

fn event_count(script: &UiScript, event: &str) -> i64 {
    lua_i64(script, &format!("ProbeMail.ev['{event}'] or 0"))
}

fn last_message(script: &UiScript) -> String {
    lua_str(script, "ProbeMail.msgs[#ProbeMail.msgs] or ''")
}

fn run(script: &UiScript, chunk: &str) -> bool {
    match script.run(chunk) {
        Ok(()) => true,
        Err(e) => {
            error!("PROBE_MAIL: Lua `{chunk}` errored: {e}");
            false
        }
    }
}

fn say(net: &NetCommands, text: String) {
    let _ = net.0.send(ClientCommand::Chat {
        kind: ChatKind::Say,
        target: None,
        text,
        language: None,
    });
}

/// The 1-based row of a message id, `None` once it is gone.
fn index_of(mail: &MailOpen, id: u32) -> Option<usize> {
    mail.index_of(id).map(|i| i + 1)
}

/// The first row whose subject is `subject`.
fn row_by_subject(mail: &MailOpen, subject: &str) -> Option<u32> {
    mail.rows
        .iter()
        .find(|r| r.entry.subject == subject)
        .map(|r| r.entry.message_id)
}

fn log_rows(mail: &MailOpen, script: &UiScript) {
    for (i, r) in mail.rows.iter().enumerate() {
        let e = &r.entry;
        let idx = i + 1;
        info!(
            "PROBE_MAIL: row {idx} id {} type {} sender {:?} subject {:?} money {} cod {} items {} \
             flags {:#x} days {:.2} body {:?}",
            e.message_id,
            e.message_type,
            lua_str(script, &format!("select(3, GetInboxHeaderInfo({idx}))")),
            lua_str(script, &format!("select(4, GetInboxHeaderInfo({idx}))")),
            e.money,
            e.cod,
            r.num_attachments(),
            e.flags,
            e.expire_days,
            e.body.chars().take(48).collect::<String>(),
        );
    }
}

fn mail_probe(
    time: ProbeClock,
    mut probe: ResMut<MailProbe>,
    mut mail: ResMut<MailOpen>,
    script: Option<NonSendMut<UiScript>>,
    self_player: Query<(), With<SelfPlayer>>,
    player: Res<Player>,
    objects: Query<(&Guid, &NetEntity, &ObjectStore, &Transform), Without<SelfPlayer>>,
    net: Res<NetCommands>,
    loading: Res<crate::loading_screen::LoadingScreen>,
    mut exit: MessageWriter<AppExit>,
) {
    if self_player.is_empty() {
        return;
    }
    let clock = time.elapsed_secs_f64();
    if loading.covering() {
        probe.clear_since = None;
        return;
    }
    let clear_since = *probe.clear_since.get_or_insert(clock);
    if probe.step == Step::Init && clock - clear_since < SETTLE_SECS {
        return;
    }
    let Some(script) = script else {
        return;
    };
    let now = time.elapsed_secs_f64();

    // The arrival trace: every change of `HasNewMail()` and the senders, timestamped.
    if probe.step != Step::Init {
        let lit = lua_bool(&script, "HasNewMail()");
        let senders = lua_str(
            &script,
            "table.concat((function(...) local t = {} for i = 1, select('#', ...) do \
             t[i] = tostring((select(i, ...))) end return t end)(GetLatestThreeSenders()), ', ')",
        );
        if probe.traced.as_ref() != Some(&(lit, senders.clone())) {
            info!("PROBE_MAIL: t={now:.2} HasNewMail={lit} senders=[{senders}]");
            probe.traced = Some((lit, senders));
        }
    }

    let step = probe.step;
    match step {
        Step::Init => {
            probe.role = Role::from_env();
            probe.me = crate::run_mode::fast_path_character().unwrap_or_default();
            if probe.me.is_empty() {
                probe.fail("(init) — WOW_CHAR is unset");
                probe.go(Step::Done, now);
                return;
            }
            run(&script, HOOKS);
            let me = probe.me.clone();
            let [x, y, z] = MAILBOX_AT;
            match &probe.role {
                Role::Solo => {
                    info!("PROBE_MAIL: solo — GM-mailing {me} a letter, money and two stacks");
                    // Earlier runs' copies and stacks out of the bags, so the copy has room.
                    for text in [
                        format!(".additem {PLAIN_LETTER} -40"),
                        format!(".additem {LINEN} -200"),
                        format!(".additem {SILK} -200"),
                        // Other probes' loot: wolf meat and rough stone.
                        ".additem 2672 -40".to_string(),
                        ".additem 2835 -40".to_string(),
                    ] {
                        say(&net, text);
                    }
                    for text in [
                        format!(".send mail {me} \"probe letter\" \"hello\""),
                        format!(".send money {me} \"probe money\" \"coins\" {MONEY_COPPER}"),
                        format!(
                            ".send items {me} \"probe items\" \"two stacks\" {LINEN}:5 {SILK}:2"
                        ),
                    ] {
                        say(&net, text);
                    }
                }
                Role::Send(peer) => {
                    info!("PROBE_MAIL: send — {me} mails {peer} two stacks, then money");
                    // A GM's mail goes out on GM stationery and reads as the GM name; mail as a
                    // player, and restore GM mode at the end.
                    for text in [
                        ".gm off".to_string(),
                        format!(".additem {LINEN} 10"),
                        format!(".additem {SILK} 4"),
                        ".modify money 100000".to_string(),
                    ] {
                        say(&net, text);
                    }
                }
                Role::Receive(peer) => {
                    info!("PROBE_MAIL: receive — {me} waits for mail from {peer:?}");
                }
            }
            // A `.go` re-enters the world even onto the spot already stood on, and the world
            // enter resets the mail session, so only a character away from the mailbox moves.
            if player
                .pos
                .distance(benilla_assets::coords::wow_to_bevy([x, y, z]))
                > MAILBOX_SCAN_RANGE
            {
                say(&net, format!(".go xyz {x} {y} {z} 0"));
            }
            probe.go(Step::FindMailbox, now);
        }
        Step::FindMailbox => {
            if now - probe.since < SETTLE_SECS {
                return;
            }
            let me = player.pos;
            let found = objects.iter().find(|(_, n, s, tf)| {
                n.kind == EntityKind::GameObject
                    && s.0.gameobject_type_id() == GO_TYPE_MAILBOX
                    && tf.translation.distance(me) < MAILBOX_SCAN_RANGE
            });
            if let Some((guid, ..)) = found {
                info!("PROBE_MAIL: mailbox {:#x} in range, using it", guid.0);
                probe.baseline = event_count(&script, "MAIL_INBOX_UPDATE");
                probe.mailbox = Some(guid.0);
                mail.click(guid.0);
                probe.step = Step::WaitList;
                probe.since = now;
            } else if probe.timed_out(now, SCAN_TIMEOUT_SECS) {
                probe.fail("(a) — no mailbox GameObject streamed in");
                probe.go(Step::Done, now);
            }
        }
        Step::WaitList => {
            if event_count(&script, "MAIL_INBOX_UPDATE") > probe.baseline
                && lua_bool(&script, "MailFrame:IsShown()")
            {
                let (n, total) = (
                    lua_i64(&script, "(GetInboxNumItems())"),
                    lua_i64(&script, "select(2, GetInboxNumItems())"),
                );
                probe.pass(&format!(
                    "(a) — MAIL_SHOW opened the stock window and the list landed: {n} shown of \
                     {total}"
                ));
                log_rows(&mail, &script);
                probe.first_list_at.get_or_insert(now);
                let next = match probe.role {
                    Role::Solo => Step::Header,
                    Role::Send(_) => Step::Attach,
                    Role::Receive(_) => Step::ReadAll,
                };
                probe.go(next, now);
            } else if probe.timed_out(now, ACTION_TIMEOUT_SECS) {
                probe.fail("(a) — no MAIL_INBOX_UPDATE with the window shown");
                probe.go(Step::Done, now);
            }
        }

        // ── Solo ──
        Step::Header => {
            let Some(id) = row_by_subject(&mail, "probe items") else {
                if probe.timed_out(now, ACTION_TIMEOUT_SECS) {
                    probe.fail("(h) — the GM items mail is not listed");
                    probe.go(Step::ReadLetter, now);
                }
                return;
            };
            let idx = index_of(&mail, id).unwrap_or(0);
            let arity = script
                .arity(&format!("GetInboxHeaderInfo({idx})"))
                .unwrap_or(0);
            let count = lua_i64(&script, &format!("select(8, GetInboxHeaderInfo({idx}))"));
            let qty = lua_str(&script, &format!("select(14, GetInboxHeaderInfo({idx}))"));
            let names: Vec<String> = (1..=16)
                .map(|j| lua_str(&script, &format!("(GetInboxItem({idx}, {j}))")))
                .filter(|n| n != "nil")
                .collect();
            if arity == 14 && count == 2 && qty == "nil" && names.len() == 2 {
                probe.pass(&format!(
                    "(h) — 14-value header, itemCount 2, no single quantity, attachments {names:?}"
                ));
            } else {
                probe.fail(&format!(
                    "(h) — header arity {arity} count {count} qty {qty} attachments {names:?}"
                ));
            }
            probe.go(Step::ReadLetter, now);
        }
        Step::ReadLetter => {
            let Some(id) = row_by_subject(&mail, "probe letter") else {
                probe.fail("(b) — the plain letter is not listed");
                probe.go(Step::TakeMoney, now);
                return;
            };
            probe.target = Some(id);
            let idx = index_of(&mail, id).unwrap_or(0);
            let body = lua_str(&script, &format!("(GetInboxText({idx}))"));
            // The VM marks the row read at once; the app's row follows when the packet goes out.
            let read_now = lua_bool(&script, &format!("select(9, GetInboxHeaderInfo({idx}))"));
            if body == "hello" && read_now {
                probe.pass("(b) — the inline body read and the row marked read at once");
            } else {
                probe.fail(&format!("(b) — body {body:?}, read {read_now}"));
            }
            probe.go(Step::CopyLetter, now);
        }
        Step::CopyLetter => {
            let Some(id) = probe.target else {
                probe.go(Step::TakeMoney, now);
                return;
            };
            let copied = |m: &MailOpen| {
                m.index_of(id)
                    .is_some_and(|i| m.rows[i].entry.flags & mail_flags::COPIED != 0)
            };
            if !probe.sent {
                if !probe.ready(now) {
                    return;
                }
                if copied(&mail) {
                    warn!("PROBE_MAIL: SKIP (b2) — the letter was copied by an earlier run");
                    probe.go(Step::ReadCopy, now);
                    return;
                }
                let idx = index_of(&mail, id).unwrap_or(0);
                run(&script, &format!("TakeInboxTextItem({idx})"));
                probe.sent = true;
            } else if copied(&mail) {
                probe.pass("(b2) — MADE_PERMANENT set the row's copied flag");
                probe.go(Step::ReadCopy, now);
            } else if probe.timed_out(now, ACTION_TIMEOUT_SECS) {
                let msg = last_message(&script);
                probe.fail(&format!(
                    "(b2) — no MADE_PERMANENT result (last message {msg:?})"
                ));
                probe.go(Step::ReadCopy, now);
            }
        }
        Step::ReadCopy => {
            if !probe.sent {
                let found = lua_i64(
                    &script,
                    &format!(
                        "(function() for bag = 0, 4 do for slot = 1, (GetContainerNumSlots(bag) \
                         or 0) do local l = GetContainerItemLink(bag, slot) if l and \
                         string.find(l, 'item:{PLAIN_LETTER}', 1, true) then return bag * 100 + \
                         slot end end end return -1 end)()"
                    ),
                );
                if found >= 0 {
                    let (bag, slot) = (found / 100, found % 100);
                    run(&script, &format!("UseContainerItem({bag}, {slot})"));
                    probe.baseline = found;
                    probe.sent = true;
                } else if probe.timed_out(now, ACTION_TIMEOUT_SECS) {
                    let bags = lua_str(
                        &script,
                        "(function() local t = {} for bag = 0, 4 do for slot = 1, \
                         (GetContainerNumSlots(bag) or 0) do local tex, n = \
                         GetContainerItemInfo(bag, slot) local l = GetContainerItemLink(bag, slot) \
                         if tex or l then table.insert(t, bag .. ':' .. slot .. '=' .. \
                         tostring(l and string.match(l, 'item:(%d+)')) .. 'x' .. tostring(n)) end \
                         end end return table.concat(t, ' ') end)()",
                    );
                    probe.fail(&format!("(b3) — no Plain Letter in the bags: {bags}"));
                    probe.go(Step::TakeMoney, now);
                }
            } else if lua_bool(
                &script,
                "ItemTextFrame:IsShown() and string.find(ItemTextGetText() or '', 'hello', 1, true)",
            ) {
                probe.pass("(b3) — the copy read by its item guid (CMSG_ITEM_TEXT_QUERY)");
                let (bag, slot) = (probe.baseline / 100, probe.baseline % 100);
                crate::ui_script::run_or_warn(&script, "CloseItemText()");
                crate::ui_script::run_or_warn(
                    &script,
                    &format!("PickupContainerItem({bag}, {slot}) DeleteCursorItem()"),
                );
                // The reader took the interaction and closed the mailbox: use it again.
                if let Some(guid) = probe.mailbox {
                    mail.click(guid);
                }
                probe.go(Step::TakeMoney, now);
            } else if probe.timed_out(now, ACTION_TIMEOUT_SECS) {
                let shown = lua_bool(&script, "ItemTextFrame:IsShown()");
                let text = lua_str(&script, "ItemTextGetText()");
                probe.fail(&format!(
                    "(b3) — the reader never showed the letter (shown {shown}, text {text:?})"
                ));
                if let Some(guid) = probe.mailbox {
                    mail.click(guid);
                }
                probe.go(Step::TakeMoney, now);
            }
        }
        Step::TakeMoney => {
            if !probe.sent {
                if !probe.ready(now) {
                    return;
                }
                let Some(id) = mail
                    .rows
                    .iter()
                    .find(|r| r.entry.money > 0 && r.entry.subject == "probe money")
                    .map(|r| r.entry.message_id)
                else {
                    probe.fail("(c) — no money row");
                    probe.go(Step::TakeItem, now);
                    return;
                };
                probe.target = Some(id);
                let idx = index_of(&mail, id).unwrap_or(0);
                run(&script, &format!("TakeInboxMoney({idx})"));
                probe.sent = true;
            } else {
                let id = probe.target.unwrap_or(0);
                let money = mail.index_of(id).map(|i| mail.rows[i].entry.money);
                if matches!(money, None | Some(0)) {
                    probe.pass(&format!(
                        "(c) — the money came out (row {})",
                        if money.is_none() { "purged" } else { "kept" }
                    ));
                    probe.go(Step::TakeItem, now);
                } else if probe.timed_out(now, ACTION_TIMEOUT_SECS) {
                    probe.fail("(c) — no MONEY_TAKEN result");
                    probe.go(Step::TakeItem, now);
                }
            }
        }
        Step::TakeItem => {
            let Some(id) = row_by_subject(&mail, "probe items") else {
                probe.fail("(d) — the items mail is gone");
                probe.go(Step::SendSelf, now);
                return;
            };
            let idx = index_of(&mail, id).unwrap_or(0);
            if !probe.sent {
                if !probe.ready(now) {
                    return;
                }
                probe.target = Some(id);
                probe.baseline = mail.rows[idx - 1].num_attachments() as i64;
                let slot = lua_i64(
                    &script,
                    &format!(
                        "(function() for j = 1, 16 do if GetInboxItem({idx}, j) then return j end \
                         end return 0 end)()"
                    ),
                );
                run(&script, &format!("TakeInboxItem({idx}, {slot})"));
                probe.sent = true;
            } else if (mail.rows[idx - 1].num_attachments() as i64) < probe.baseline {
                probe.pass("(d) — one attachment taken by slot, the rest kept");
                probe.go(Step::AutoLoot, now);
            } else if probe.timed_out(now, ACTION_TIMEOUT_SECS) {
                let msg = last_message(&script);
                probe.fail(&format!(
                    "(d) — no ITEM_TAKEN result (last message {msg:?})"
                ));
                probe.go(Step::AutoLoot, now);
            }
        }
        Step::AutoLoot => {
            let id = probe.target.unwrap_or(0);
            let left = mail.index_of(id).map(|i| mail.rows[i].num_attachments());
            if !probe.sent {
                if !probe.ready(now) {
                    return;
                }
                let idx = index_of(&mail, id).unwrap_or(0);
                run(&script, &format!("AutoLootMailItem({idx})"));
                probe.sent = true;
            } else if matches!(left, None | Some(0)) {
                probe.pass("(d2) — AutoLootMailItem emptied the rest");
                probe.go(Step::SendSelf, now);
            } else if probe.timed_out(now, ACTION_TIMEOUT_SECS) {
                probe.fail(&format!("(d2) — {left:?} attachment(s) left"));
                probe.go(Step::SendSelf, now);
            }
        }
        Step::SendSelf | Step::SendBad => {
            let (target, want, tag, next) = if step == Step::SendSelf {
                (probe.me.clone(), "yourself", "(e)", Step::SendBad)
            } else {
                (
                    "Zzznonexistent".to_string(),
                    "recipient",
                    "(f)",
                    Step::Delete,
                )
            };
            if !probe.sent {
                if !probe.ready(now) {
                    return;
                }
                probe.baseline = event_count(&script, "MAIL_FAILED");
                run(
                    &script,
                    &format!("SendMail('{target}', 'probe refusal', 'x')"),
                );
                probe.sent = true;
            } else if event_count(&script, "MAIL_FAILED") > probe.baseline {
                let text = last_message(&script);
                if text.to_lowercase().contains(want) {
                    probe.pass(&format!("{tag} — refused with {text:?} and MAIL_FAILED"));
                } else {
                    probe.fail(&format!("{tag} — refused with {text:?}"));
                }
                probe.go(next, now);
            } else if probe.timed_out(now, ACTION_TIMEOUT_SECS) {
                probe.fail(&format!("{tag} — no MAIL_FAILED"));
                probe.go(next, now);
            }
        }
        Step::Delete => {
            let found = if probe.sent {
                probe.target
            } else {
                row_by_subject(&mail, "probe letter")
            };
            let Some(id) = found else {
                probe.fail("(g) — the letter is already gone");
                probe.go(Step::Close, now);
                return;
            };
            if !probe.sent {
                if !probe.ready(now) {
                    return;
                }
                let idx = index_of(&mail, id).unwrap_or(0);
                if !lua_bool(&script, &format!("InboxItemCanDelete({idx})")) {
                    probe.fail("(g) — InboxItemCanDelete says no for an empty read letter");
                }
                run(&script, &format!("DeleteInboxItem({idx})"));
                probe.target = Some(id);
                probe.sent = true;
            } else if probe.timed_out(now, ACTION_TIMEOUT_SECS) {
                probe.fail("(g) — the letter is still listed");
                probe.go(Step::Close, now);
            }
            if probe.sent && mail.index_of(id).is_none() {
                probe.pass("(g) — DELETED removed the row");
                probe.go(Step::Close, now);
            }
        }
        Step::Close => {
            if !probe.sent {
                probe.baseline = event_count(&script, "MAIL_CLOSED");
                run(&script, "CloseMail()");
                probe.sent = true;
            } else if event_count(&script, "MAIL_CLOSED") > probe.baseline
                && !lua_bool(&script, "MailFrame:IsShown()")
            {
                probe.pass("(i) — CloseMail fired MAIL_CLOSED and hid the window");
                probe.go(Step::Done, now);
            } else if probe.timed_out(now, ACTION_TIMEOUT_SECS) {
                probe.fail("(i) — no MAIL_CLOSED");
                probe.go(Step::Done, now);
            }
        }

        // ── Send ──
        Step::Attach => {
            if !probe.sent {
                run(&script, "MailFrameTab_OnClick(nil, 2)");
                for item in [LINEN, SILK] {
                    run(
                        &script,
                        &format!(
                            "for bag = 0, 4 do for slot = 1, (GetContainerNumSlots(bag) or 0) do \
                             local l = GetContainerItemLink(bag, slot) if l and string.find(l, \
                             'item:{item}', 1, true) then UseContainerItem(bag, slot) return end \
                             end end"
                        ),
                    );
                }
                probe.sent = true;
            } else if lua_bool(&script, "GetSendMailItem(1) and GetSendMailItem(2)") {
                let price = lua_i64(&script, "GetSendMailPrice()");
                let locked = lua_bool(
                    &script,
                    "(function() for bag = 0, 4 do for slot = 1, (GetContainerNumSlots(bag) or \
                     0) do local _, _, l = GetContainerItemInfo(bag, slot) if l then return true \
                     end end end end)()",
                );
                if price == 60 && locked {
                    probe.pass("(s0) — two stacks attached by bag click, slots locked, postage 60");
                } else {
                    probe.fail(&format!("(s0) — postage {price}, a slot locked {locked}"));
                }
                probe.go(Step::SendItems, now);
            } else if probe.timed_out(now, ACTION_TIMEOUT_SECS) {
                probe.fail("(s0) — the attachments never showed");
                probe.go(Step::SendMoney, now);
            }
        }
        Step::SendItems | Step::SendMoney => {
            let Role::Send(peer) = probe.role.clone() else {
                return;
            };
            let (subject, money, tag, next) = if step == Step::SendItems {
                ("probe stacks", 0, "(s1)", Step::SendMoney)
            } else {
                ("probe coins", SEND_COPPER, "(s2)", Step::Done)
            };
            if !probe.sent {
                if !probe.ready(now) {
                    return;
                }
                probe.baseline = event_count(&script, "MAIL_SUCCESS");
                let me = probe.me.clone();
                run(
                    &script,
                    &format!(
                        "SendMailNameEditBox:SetText('{peer}') \
                         SendMailSubjectEditBox:SetText('{subject}') \
                         SendMailBodyEditBox:SetText('from {me}') \
                         SetSendMailMoney({money}) SendMailFrame_SendMail()"
                    ),
                );
                probe.sent = true;
            } else if event_count(&script, "MAIL_SUCCESS") > probe.baseline {
                let text = last_message(&script);
                let reset = lua_bool(&script, "not GetSendMailItem(1)");
                if reset {
                    probe.pass(&format!(
                        "{tag} — mailed {peer} {subject:?}: {text:?}, MAIL_SUCCESS, form reset"
                    ));
                } else {
                    probe.fail(&format!("{tag} — sent but the form kept its attachment"));
                }
                probe.go(next, now);
            } else if event_count(&script, "MAIL_FAILED") > 0 && probe.timed_out(now, 2.0) {
                probe.fail(&format!("{tag} — refused: {:?}", last_message(&script)));
                probe.go(next, now);
            } else if probe.timed_out(now, ACTION_TIMEOUT_SECS) {
                probe.fail(&format!("{tag} — no MAIL_SUCCESS"));
                probe.go(next, now);
            }
        }

        // ── Receive ──
        Step::ReadAll => {
            let n = mail.rows.len();
            for idx in 1..=n {
                crate::ui_script::run_or_warn(&script, &format!("GetInboxText({idx})"));
            }
            probe.seen = mail.rows.iter().map(|r| r.entry.message_id).collect();
            info!("PROBE_MAIL: (r0) read all {n} row(s); closing to re-ask the server");
            run(&script, "CloseMail()");
            probe.go(Step::WaitDark, now);
        }
        Step::WaitDark => {
            if !lua_bool(&script, "HasNewMail()") && probe.timed_out(now, 2.0) {
                probe.pass("(r0) — everything read: the re-query darkened the icon");
                info!("PROBE_MAIL: READY t={now:.2} — waiting for an arrival");
                probe.go(Step::WaitArrival, now);
            } else if probe.timed_out(now, ACTION_TIMEOUT_SECS) {
                probe.fail("(r0) — the icon stayed lit after reading everything");
                probe.go(Step::WaitArrival, now);
            }
        }
        Step::WaitArrival => {
            let wait = std::env::var("WOW_PROBE_MAIL_WAIT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(DEFAULT_WAIT_SECS);
            let peer = match &probe.role {
                Role::Receive(p) => p.clone(),
                _ => None,
            };
            let lit = lua_bool(&script, "HasNewMail()");
            let senders = lua_str(
                &script,
                "table.concat((function(...) local t = {} for i = 1, select('#', ...) do \
                 t[i] = tostring((select(i, ...))) end return t end)(GetLatestThreeSenders()), \
                 '|')",
            );
            let named = !senders.is_empty() && !senders.contains("nil");
            let from_peer = peer.as_ref().is_none_or(|p| senders.contains(p.as_str()));
            if lit && named && from_peer {
                run(
                    &script,
                    "MiniMapMailFrame:GetScript('OnEnter')(MiniMapMailFrame)",
                );
                let tip = lua_str(&script, "GameTooltipTextLeft1:GetText()");
                let shown = lua_bool(&script, "MiniMapMailFrame:IsShown()");
                let head = lua_str(&script, "HAVE_MAIL_FROM");
                let lines: Vec<&str> = senders.split('|').collect();
                if shown && tip.starts_with(&head) && lines.iter().all(|s| tip.contains(s)) {
                    probe.pass(&format!(
                        "(r1) — t={now:.2} the icon lit; its tooltip reads {tip:?}"
                    ));
                } else {
                    probe.fail(&format!(
                        "(r1) — icon shown {shown}, tooltip {tip:?}, senders {senders:?}"
                    ));
                }
                crate::ui_script::run_or_warn(&script, "GameTooltip:Hide()");
                probe.go(Step::WaitRelist, now);
            } else if probe.timed_out(now, wait) {
                probe.fail(&format!(
                    "(r1) — nothing arrived in {wait}s (lit {lit}, senders {senders:?})"
                ));
                probe.go(Step::Done, now);
            }
        }
        Step::WaitRelist => {
            // A re-list inside a minute of the last answers the cached rows, as `CheckInbox`'s
            // throttle does.
            let since_list = now - probe.first_list_at.unwrap_or(now);
            if since_list >= RELIST_SECS {
                probe.target = Some(1);
                probe.baseline = event_count(&script, "MAIL_INBOX_UPDATE");
                if let Some(guid) = objects
                    .iter()
                    .find(|(_, n, s, tf)| {
                        n.kind == EntityKind::GameObject
                            && s.0.gameobject_type_id() == GO_TYPE_MAILBOX
                            && tf.translation.distance(player.pos) < MAILBOX_SCAN_RANGE
                    })
                    .map(|(g, ..)| g.0)
                {
                    mail.click(guid);
                }
                probe.step = Step::ReadNew;
                probe.since = now;
            }
        }
        Step::ReadNew => {
            let fresh: Vec<usize> = mail
                .rows
                .iter()
                .enumerate()
                .filter(|(_, r)| !probe.seen.contains(&r.entry.message_id))
                .map(|(i, _)| i + 1)
                .collect();
            if event_count(&script, "MAIL_INBOX_UPDATE") <= probe.baseline || fresh.is_empty() {
                if probe.timed_out(now, ACTION_TIMEOUT_SECS) {
                    probe.fail("(r2) — the re-list shows nothing new");
                    probe.go(Step::Done, now);
                }
                return;
            }
            if !probe.sent {
                log_rows(&mail, &script);
                for idx in &fresh {
                    let body = lua_str(&script, &format!("(GetInboxText({idx}))"));
                    let invoice = lua_str(
                        &script,
                        &format!(
                            "table.concat({{tostring((GetInboxInvoiceInfo({idx}))), \
                             tostring(select(2, GetInboxInvoiceInfo({idx}))), \
                             tostring(select(3, GetInboxInvoiceInfo({idx}))), \
                             tostring(select(4, GetInboxInvoiceInfo({idx})))}}, ' ')"
                        ),
                    );
                    info!("PROBE_MAIL: new row {idx}: body {body:?} invoice [{invoice}]");
                }
                // The first new row with money pays out.
                if let Some(idx) = fresh.iter().find(|&&i| mail.rows[i - 1].entry.money > 0) {
                    probe.target = Some(mail.rows[idx - 1].entry.message_id);
                    run(&script, &format!("TakeInboxMoney({idx})"));
                } else {
                    probe.target = None;
                }
                probe.sent = true;
                return;
            }
            let paid = probe.target.is_none_or(|id| {
                mail.index_of(id)
                    .is_none_or(|i| mail.rows[i].entry.money == 0)
            });
            if paid {
                let took = if probe.target.is_some() {
                    ", the money taken"
                } else {
                    ""
                };
                probe.pass(&format!(
                    "(r2) — {} new row(s) listed and read{took}",
                    fresh.len()
                ));
                run(&script, "CloseMail()");
                probe.go(Step::Done, now);
            } else if probe.timed_out(now, ACTION_TIMEOUT_SECS) {
                probe.fail("(r2) — the money take never resolved");
                probe.go(Step::Done, now);
            }
        }
        Step::Done => {
            if probe.exited {
                return;
            }
            probe.exited = true;
            if matches!(probe.role, Role::Send(_)) {
                say(&net, ".gm on".to_string());
            }
            info!(
                "PROBE_MAIL: DONE pass={} fail={}",
                probe.passes, probe.fails
            );
            exit.write(AppExit::Success);
            std::thread::spawn(|| {
                std::thread::sleep(std::time::Duration::from_secs(5));
                warn!("PROBE_MAIL: still alive 5s after AppExit — hard exit");
                std::process::exit(0);
            });
        }
    }
}
