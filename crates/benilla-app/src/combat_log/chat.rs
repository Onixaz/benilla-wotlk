//! The chat lines 3.3.5a still prints beside its combat log, queued for their names; the wording
//! law is [`crate::ui_chat::combat`].

use crate::ui_chat::combat::{self, Fills, Named, PendingCombat};
use crate::ui_chat::{ChatEventKind, ChatLog};
use benilla_ui::script::ScriptValue;

/// A line whose subject is `subject`, keyed on it being the player.
fn line(
    kind: ChatEventKind,
    family: combat::Family,
    me: Option<u64>,
    subject: u64,
    fills: Fills,
    named: Named,
) -> PendingCombat {
    let me = me == Some(subject);
    PendingCombat {
        kind,
        family,
        me,
        subject: if me { 0 } else { subject },
        fills,
        named,
        tries: 0,
    }
}

/// The tradeskill-create line (`0x50bd90`): the created item, `CHAT_MSG_TRADESKILLS`.
pub(crate) fn tradeskill_create(log: &mut ChatLog, me: Option<u64>, caster: u64, item: u32) {
    log.push_combat(line(
        ChatEventKind::Tradeskills,
        combat::TRADESKILL_LOG,
        me,
        caster,
        Fills::default(),
        Named::Item(item),
    ));
}

/// The feed-pet line (`0x50baf0`): the food, `CHAT_MSG_PET_INFO`.
pub(crate) fn feed_pet(log: &mut ChatLog, me: Option<u64>, caster: u64, item: u32) {
    log.push_combat(line(
        ChatEventKind::PetInfo,
        combat::FEEDPET_LOG,
        me,
        caster,
        Fills::default(),
        Named::Item(item),
    ));
}

/// The dismiss-pet line (`0x50bcc0`): the pet, `CHAT_MSG_PET_INFO`.
pub(crate) fn dismiss_pet(log: &mut ChatLog, me: Option<u64>, caster: u64, pet: u64) {
    log.push_combat(line(
        ChatEventKind::PetInfo,
        combat::SPELLDISMISSPET,
        me,
        caster,
        Fills::default(),
        Named::Unit(pet),
    ));
}

/// The pet-happiness line (`0x50bbf0`), the happiness drain in place of its entry: the pet and
/// the shown amount, `CHAT_MSG_PET_INFO`.
pub(crate) fn happiness_drain(
    log: &mut ChatLog,
    me: Option<u64>,
    caster: u64,
    pet: u64,
    amount: u32,
) {
    log.push_combat(line(
        ChatEventKind::PetInfo,
        combat::SPELLHAPPINESSDRAIN,
        me,
        caster,
        Fills {
            amount: i64::from(amount),
            ..Fills::default()
        },
        Named::Unit(pet),
    ));
}

/// What a lock opening opened.
pub(crate) enum Opened {
    /// An item entry, named off the item cache.
    Item(u32),
    /// A game object's name, already cached.
    GameObject(String),
}

/// The open-lock line (`0x50dde0`, `0x50deb0`): the spell and what it opened, `CHAT_MSG_OPENING`.
pub(crate) fn open_lock(
    log: &mut ChatLog,
    me: Option<u64>,
    caster: u64,
    spell: String,
    opened: Opened,
) {
    let (named, text) = match opened {
        Opened::Item(entry) => (Named::Item(entry), String::new()),
        Opened::GameObject(name) => (Named::Ready, name),
    };
    log.push_combat(line(
        ChatEventKind::Opening,
        combat::OPEN_LOCK,
        me,
        caster,
        Fills {
            spell,
            named: text,
            ..Fills::default()
        },
        named,
    ));
}

/// The faction-change line (`0x50b8c0`). The generic form (`0x50b90c`) names the faction alone
/// and sends no combat text; otherwise a rise with a bonus above 0 takes the bonus form
/// (`0x50b96f`), and the line is followed by the `FACTION` combat text (`0x50b9f4`), returned.
pub(crate) fn faction_change_line(
    name: &str,
    delta: i32,
    generic: bool,
    bonus: f32,
    log: &mut ChatLog,
) -> Option<crate::ui_unit::CombatTextEvent> {
    if delta == 0 {
        return None;
    }
    let family = match (generic, delta > 0) {
        (true, true) => combat::FACTION_STANDING_INCREASED_GENERIC,
        (true, false) => combat::FACTION_STANDING_DECREASED_GENERIC,
        (false, true) if bonus > 0.0 => combat::FACTION_STANDING_INCREASED_BONUS,
        (false, true) => combat::FACTION_STANDING_INCREASED,
        (false, false) => combat::FACTION_STANDING_DECREASED,
    };
    log.push_combat(combat::ready(
        ChatEventKind::CombatFactionChange,
        family,
        Fills {
            named: name.to_string(),
            amount: i64::from(delta.abs()),
            bonus: f64::from(bonus),
            ..Fills::default()
        },
    ));
    (!generic).then(|| crate::ui_unit::CombatTextEvent {
        message_type: "FACTION",
        args: vec![
            ScriptValue::Str(name.to_string()),
            ScriptValue::Int(i64::from(delta)),
        ],
    })
}

/// The rank-change line (`0x50ba10`), a system line: `FACTION_STANDING_LABEL<rank + 1>` in the
/// player's gender (`0x50ba8e`), then the faction.
pub(crate) fn faction_rank_line(name: &str, rank: u8, female: bool, log: &mut ChatLog) {
    let label = format!(
        "FACTION_STANDING_LABEL{}{}",
        rank + 1,
        if female { "_FEMALE" } else { "" }
    );
    log.push_combat(combat::ready(
        ChatEventKind::System,
        combat::FACTION_STANDING_CHANGED,
        Fills {
            named: name.to_string(),
            label,
            ..Fills::default()
        },
    ));
}
