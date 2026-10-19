//! The chat lines 3.3.5a still prints beside its combat log (`CGChat::Log*`), queued for their
//! names; the wording law is [`crate::ui_chat::combat`].

use crate::net::Reputations;
use crate::ui_chat::combat::{self, Fills, Named, PendingCombat};
use crate::ui_chat::{ChatEventKind, ChatLog};

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

/// `LogTradeSkillCreate` (`0x50bd90`): the created item, `CHAT_MSG_TRADESKILLS`.
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

/// `LogFeedPet` (`0x50baf0`): the food, `CHAT_MSG_PET_INFO`.
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

/// `LogDismissPet` (`0x50bcc0`): the pet, `CHAT_MSG_PET_INFO`.
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

/// `LogPetHappinessDecrease` (`0x50bbf0`), the happiness drain in place of its entry: the pet and
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

/// `LogOpenLock` (`0x50dde0`, `0x50deb0`): the spell and what it opened, `CHAT_MSG_OPENING`.
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

/// `SMSG_SET_FACTION_STANDING` through `LogFactionChanged` (`0x50b8c0`): the faction and the
/// delta. The wire carries the new total, so this runs before the store is overwritten, and an
/// unchanged value prints nothing. The wire's list id is `Faction.dbc`'s `rep_index`.
pub(crate) fn faction_standing(
    deltas: &[(u32, i32)],
    reputations: &Reputations,
    factions: Option<&crate::target::ring::Factions>,
    log: &mut ChatLog,
) {
    let Some(catalog) = factions.map(|f| f.catalog()) else {
        return;
    };
    for &(list_id, standing) in deltas {
        let old = reputations
            .0
            .get(list_id as usize)
            .map_or(0, |(_, standing)| *standing);
        let delta = standing - old;
        if delta == 0 {
            continue;
        }
        let Ok(index) = i32::try_from(list_id) else {
            continue;
        };
        let Some(name) = catalog
            .reputation_factions()
            .find(|(_, f)| f.rep_index == index)
            .and_then(|(id, _)| catalog.faction_name(id))
        else {
            continue;
        };
        log.push_combat(combat::ready(
            ChatEventKind::CombatFactionChange,
            if delta > 0 {
                combat::FACTION_STANDING_INCREASED
            } else {
                combat::FACTION_STANDING_DECREASED
            },
            Fills {
                named: name.to_string(),
                amount: i64::from(delta.abs()),
                ..Fills::default()
            },
        ));
    }
}
