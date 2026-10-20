//! The mail arrival layer: the reference's next-mail time behind `HasNewMail()` and the three
//! recent senders behind `GetLatestThreeSenders()`. It outlives the mailbox window.

use benilla_protocol::messages::{MailSender, MAIL_RECENT_SENDERS};
use bevy::prelude::*;

/// The epsilon `HasNewMail()` (`0x56f350`) and the per-frame step (`0x56ded0`) compare `fabs`
/// against: `[0x9ea27c]`, 2^-22.
const MAIL_TIME_EPSILON: f32 = f32::from_bits(0x3480_0000);

/// The "no mail" stamp the world enter (`0x571530`) and the query sender (`0x56ff70`) write.
const MAIL_TIME_NO_MAIL: f32 = -1.0;

/// The reference's pending-mail state: the next-mail time `[0xace60c]`, the recent senders
/// `[0xbeafe8]` and the deferred re-query flag `[0xbeb1e8]`.
#[derive(Resource)]
pub(crate) struct MailPending {
    /// Seconds until mail is waiting; near 0 lights the icon.
    countdown: f32,
    /// Up to three senders, each with its own delivery countdown; an empty slot has no guid and
    /// no entry.
    senders: [Option<MailSender>; MAIL_RECENT_SENDERS],
    /// Re-ask the server when the mailbox closes (`m_doPendingQuery`).
    do_pending_query: bool,
    /// A queued `UPDATE_PENDING_MAIL`.
    notify: bool,
}

impl Default for MailPending {
    /// No mail waiting and nothing deferred, as the world enter stamps.
    fn default() -> Self {
        Self {
            countdown: MAIL_TIME_NO_MAIL,
            senders: [None; MAIL_RECENT_SENDERS],
            do_pending_query: false,
            notify: false,
        }
    }
}

fn near_zero(v: f32) -> bool {
    v.abs() < MAIL_TIME_EPSILON
}

impl MailPending {
    /// `HasNewMail()` (`0x56f350`): `|countdown| < ε`, false at equality and for NaN.
    pub(crate) fn has_new_mail(&self) -> bool {
        near_zero(self.countdown)
    }

    /// The per-frame step (`0x56ded0`): each sender and the next-mail time count down while
    /// positive, floor at 0 and signal on the step that lands inside ε.
    pub(super) fn step(&mut self, delta_secs: f32) {
        for s in self.senders.iter_mut().flatten() {
            if s.delay > 0.0 {
                s.delay = (s.delay - delta_secs).max(0.0);
                if near_zero(s.delay) {
                    self.notify = true;
                }
            }
        }
        if self.countdown > 0.0 {
            let diff = f64::from(self.countdown) - f64::from(delta_secs);
            self.countdown = if diff > 0.0 { diff as f32 } else { 0.0 };
            if self.has_new_mail() {
                self.notify = true;
            }
        }
    }

    /// The `MSG_QUERY_NEXT_MAIL_TIME` reply (`0x56daf0`): the time and the first three senders as
    /// sent, every later slot cleared, and the signal whether or not anything changed.
    pub(crate) fn apply_query_reply(&mut self, seconds: f32, senders: &[MailSender]) {
        self.countdown = seconds;
        for (i, slot) in self.senders.iter_mut().enumerate() {
            *slot = senders.get(i).copied();
        }
        self.notify = true;
    }

    /// `QueryNextMailTime` (`0x56ff70`): the query goes out and the time stamps "no mail" with no
    /// signal, so the icon keeps its face until the reply; the deferred re-query is spent.
    pub(super) fn on_query_sent(&mut self) {
        self.countdown = MAIL_TIME_NO_MAIL;
        self.do_pending_query = false;
    }

    /// `ReceivedMail` (`0x5704d0`); `mailbox_open` is its test of the open mailbox. Answers whether
    /// the caller sends the query, which this has already stamped.
    pub(crate) fn received_mail(&mut self, delay: f32, mailbox_open: bool) -> bool {
        if mailbox_open {
            self.do_pending_query = true;
            return false;
        }
        self.on_query_sent();
        if near_zero(delay) {
            self.countdown = 0.0;
            self.notify = true;
        } else if self.countdown < 0.0 || self.countdown > delay {
            self.countdown = delay;
        }
        true
    }

    /// `MarkAsRead`'s tail (`0x56fed6`-`0x56ff3d`): a read arms the close-time re-query, and once
    /// every row is read a waiting icon goes dark.
    pub(super) fn on_mail_read(&mut self, all_read: bool) {
        if near_zero(self.countdown) && all_read {
            self.countdown = MAIL_TIME_NO_MAIL;
            self.notify = true;
        }
        self.do_pending_query = true;
    }

    /// `UpdateRecentSenders` (`0x56de00`): a sender with no unread row left drops out; any drop
    /// signals. `has_unread` answers for a `(guid, entry)`.
    pub(super) fn update_recent_senders(&mut self, has_unread: impl Fn(u64, u32) -> bool) {
        let mut changed = false;
        for slot in &mut self.senders {
            if let Some(s) = slot {
                if !has_unread(s.guid, s.entry) {
                    *slot = None;
                    changed = true;
                }
            }
        }
        if changed {
            self.notify = true;
        }
    }

    /// The senders `GetLatestThreeSenders` lists: named and delivered (`0x56f3cb`).
    pub(super) fn delivered_senders(&self) -> impl Iterator<Item = &MailSender> {
        self.senders
            .iter()
            .flatten()
            .filter(|s| (s.guid != 0 || s.entry != 0) && s.delay <= 0.0)
    }

    /// A sender's name landed (`SenderNameQueryCallback`, `0x56d47a`).
    pub(super) fn notify_senders(&mut self) {
        self.notify = true;
    }

    pub(super) fn take_do_pending_query(&mut self) -> bool {
        std::mem::take(&mut self.do_pending_query)
    }

    pub(super) fn take_notify(&mut self) -> bool {
        std::mem::take(&mut self.notify)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending_at(countdown: f32) -> MailPending {
        MailPending {
            countdown,
            ..Default::default()
        }
    }

    fn sender(guid: u64, delay: f32) -> MailSender {
        MailSender {
            guid,
            entry: 0,
            message_type: 0,
            stationery: 41,
            delay,
        }
    }

    #[test]
    fn has_new_mail_is_near_zero_not_non_positive() {
        assert!(!MailPending::default().has_new_mail());
        assert!(pending_at(0.0).has_new_mail());
        assert!(!pending_at(-86400.0).has_new_mail());
        assert!(pending_at(MAIL_TIME_EPSILON / 2.0).has_new_mail());
        assert!(!pending_at(MAIL_TIME_EPSILON).has_new_mail());
        assert!(!pending_at(f32::NAN).has_new_mail());
        assert!(!pending_at(30.0).has_new_mail());
    }

    #[test]
    fn the_step_counts_down_and_signals_once_at_zero() {
        let mut none = pending_at(-86400.0);
        none.step(1.0);
        assert_eq!(none.countdown, -86400.0);
        assert!(!none.take_notify());

        let mut counting = pending_at(0.5);
        counting.step(0.125);
        assert_eq!(counting.countdown, 0.375);
        assert!(!counting.take_notify());
        counting.step(10.0);
        assert!(counting.has_new_mail());
        assert!(counting.take_notify());
        counting.step(1.0);
        assert!(!counting.take_notify(), "a reached zero does not re-signal");
    }

    #[test]
    fn the_reply_keeps_three_senders_and_always_signals() {
        let mut p = MailPending::default();
        p.apply_query_reply(0.0, &[sender(0xA, 0.0), sender(0xB, 30.0)]);
        assert!(p.has_new_mail());
        assert!(p.take_notify());
        let listed: Vec<u64> = p.delivered_senders().map(|s| s.guid).collect();
        assert_eq!(listed, vec![0xA], "a sender still in transit is not listed");
        // Its countdown runs with the frame and signals on arrival.
        p.step(30.0);
        assert!(p.take_notify());
        assert_eq!(p.delivered_senders().count(), 2);
        // A later reply with fewer senders clears the rest.
        p.apply_query_reply(-86400.0, &[]);
        assert_eq!(p.delivered_senders().count(), 0);
    }

    #[test]
    fn received_mail_queries_first_then_keeps_the_delay() {
        // Mailbox open: only the deferred re-query.
        let mut busy = pending_at(-86400.0);
        assert!(!busy.received_mail(0.0, true));
        assert_eq!(busy.countdown, -86400.0);
        assert!(busy.take_do_pending_query());

        // No delay: lit at once.
        let mut now = pending_at(-86400.0);
        assert!(now.received_mail(0.0, false));
        assert!(now.has_new_mail());
        assert!(now.take_notify());

        // A delay: the query's stamp is negative, so the delay always takes.
        let mut later = pending_at(5.0);
        assert!(later.received_mail(3600.0, false));
        assert_eq!(later.countdown, 3600.0);
        assert!(!later.take_notify());
    }

    #[test]
    fn reading_the_last_unread_mail_darkens_the_icon() {
        let mut p = pending_at(0.0);
        p.on_mail_read(false);
        assert!(p.has_new_mail(), "unread mail is left");
        assert!(p.take_do_pending_query());
        p.on_mail_read(true);
        assert!(!p.has_new_mail());
        assert!(p.take_notify());
    }

    #[test]
    fn a_sender_with_nothing_unread_drops_out() {
        let mut p = MailPending::default();
        p.apply_query_reply(0.0, &[sender(0xA, 0.0), sender(0xB, 0.0)]);
        p.take_notify();
        p.update_recent_senders(|guid, _| guid == 0xB);
        assert_eq!(
            p.delivered_senders().map(|s| s.guid).collect::<Vec<_>>(),
            vec![0xB]
        );
        assert!(p.take_notify());
    }
}
