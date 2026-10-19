//! The combat chat lines 3.3.5a still prints beside its combat log (`CGChat::Log*`): created items,
//! pet lines, opened locks, the death's durability loss and reputation changes. Each picks a
//! GlobalString key by whether its subject is the player and fills it as `SStrPrintf` does.

use crate::names::NameCache;
use crate::net::{NetCommands, ObjectStore};
use crate::ui_chat::event::ChatEventKind;

/// How a family builds its GlobalString key: two keys off whether the subject is the player, or one
/// fixed key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Keying {
    /// The `me` word when the subject is the player, else `other`.
    Duo {
        me: &'static str,
        other: &'static str,
    },
    /// One key: the stem is the whole name.
    Single,
}

/// One value slot of a family's format string, in the order the shipped template consumes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Slot {
    /// `%s`: the subject's name, spelled only by a [`Keying::Duo`] family's `other` key.
    Subject,
    /// `%s`: the spell's name.
    Spell,
    /// `%s`: a name spelled in every variant: an item, a pet, a game object, a faction.
    Named,
    /// `%d`.
    Amount,
}

/// A line family: the stem its keys are built from, and the ordered slots its templates consume.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Family {
    pub stem: &'static str,
    pub keying: Keying,
    pub slots: &'static [Slot],
}

impl Family {
    /// The GlobalString key for a subject that is, or is not, the player.
    pub(crate) fn key(&self, me: bool) -> String {
        match self.keying {
            Keying::Duo { me: m, other } => format!("{}{}", self.stem, if me { m } else { other }),
            Keying::Single => self.stem.to_string(),
        }
    }
}

/// The values bound to each [`Slot`] for one line.
#[derive(Clone, Debug, Default)]
pub(crate) struct Fills {
    pub subject: String,
    pub spell: String,
    pub amount: i64,
    pub named: String,
}

/// Resolve a family to a finished line; `None` when the key is absent or its template disagrees
/// with the slots.
pub(crate) fn compose_line(
    script: &benilla_ui::script::UiScript,
    family: Family,
    me: bool,
    fills: &Fills,
) -> Option<String> {
    let key = family.key(me);
    let template = global_string(script, &key)?;
    if family.slots.is_empty() {
        return Some(template);
    }
    let args: Vec<Arg> = family
        .slots
        .iter()
        .filter_map(|slot| match slot {
            Slot::Subject => {
                (!me && family.keying != Keying::Single).then_some(Arg::Str(&fills.subject))
            }
            Slot::Spell => Some(Arg::Str(&fills.spell)),
            Slot::Named => Some(Arg::Str(&fills.named)),
            Slot::Amount => Some(Arg::Num(fills.amount)),
        })
        .collect();
    let line = fill(&template, &args);
    if line.is_none() {
        bevy::log::warn!(
            "chat: {key} does not match our slot order ({:?})",
            family.slots
        );
    }
    line
}

/// One `SStrPrintf` argument.
enum Arg<'a> {
    Str(&'a str),
    Num(i64),
}

/// `SStrPrintf` over the `%s`/`%d` subset these templates use; `None` on any mismatch.
fn fill(template: &str, args: &[Arg<'_>]) -> Option<String> {
    let mut out = String::with_capacity(template.len() + 32);
    let mut next = args.iter();
    let mut chars = template.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('%') => out.push('%'),
            Some('s') => match next.next()? {
                Arg::Str(s) => out.push_str(s),
                Arg::Num(_) => return None,
            },
            Some('d') => match next.next()? {
                Arg::Num(n) => out.push_str(&n.to_string()),
                Arg::Str(_) => return None,
            },
            _ => return None,
        }
    }
    next.next().is_none().then_some(out)
}

/// A GlobalString off the VM's globals, `None` when absent or empty.
pub(crate) fn global_string(script: &benilla_ui::script::UiScript, key: &str) -> Option<String> {
    script
        .lua()
        .globals()
        .get::<Option<String>>(key)
        .ok()
        .flatten()
        .filter(|s| !s.is_empty())
}

/// A unit's display name (`GetObjectNameFromGuid`); `None` until the name cache answers.
pub(crate) fn object_name(
    guid: u64,
    unit: Option<&ObjectStore>,
    names: &NameCache,
    commands: &NetCommands,
) -> Option<String> {
    if guid == 0 {
        return None;
    }
    names.resolve_unit(guid, unit, commands).map(str::to_owned)
}

macro_rules! duo {
    ($name:ident, $stem:literal, $me:literal, $other:literal, [$($slot:ident),* $(,)?], $doc:literal) => {
        #[doc = $doc]
        pub(crate) const $name: Family = Family {
            stem: $stem,
            keying: Keying::Duo { me: $me, other: $other },
            slots: &[$(Slot::$slot),*],
        };
    };
}

macro_rules! single {
    ($name:ident, $key:literal, [$($slot:ident),* $(,)?], $doc:literal) => {
        #[doc = $doc]
        pub(crate) const $name: Family = Family {
            stem: $key,
            keying: Keying::Single,
            slots: &[$(Slot::$slot),*],
        };
    };
}

duo!(
    TRADESKILL_LOG,
    "TRADESKILL_LOG",
    "_FIRSTPERSON",
    "_THIRDPERSON",
    [Subject, Named],
    "`LogTradeSkillCreate` (`0x50bd90`): the created item."
);
duo!(
    FEEDPET_LOG,
    "FEEDPET_LOG",
    "_FIRSTPERSON",
    "_THIRDPERSON",
    [Subject, Named],
    "`LogFeedPet` (`0x50baf0`): the food."
);
duo!(
    SPELLDISMISSPET,
    "SPELLDISMISSPET",
    "SELF",
    "OTHER",
    [Subject, Named],
    "`LogDismissPet` (`0x50bcc0`): the pet."
);
duo!(
    SPELLHAPPINESSDRAIN,
    "SPELLHAPPINESSDRAIN",
    "SELF",
    "OTHER",
    [Subject, Named, Amount],
    "`LogPetHappinessDecrease` (`0x50bbf0`): the pet, then the shown amount."
);
duo!(
    OPEN_LOCK,
    "OPEN_LOCK_",
    "SELF",
    "OTHER",
    [Subject, Spell, Named],
    "`LogOpenLock` (`0x50dde0`, `0x50deb0`): the spell, then the opened item or object."
);
single!(
    DURABILITYDAMAGE_DEATH,
    "DURABILITYDAMAGE_DEATH",
    [],
    "The death's durability loss, an empty body."
);
single!(
    FACTION_STANDING_INCREASED,
    "FACTION_STANDING_INCREASED",
    [Named, Amount],
    "`LogFactionChanged` (`0x50b8c0`): the faction, the delta."
);
single!(
    FACTION_STANDING_DECREASED,
    "FACTION_STANDING_DECREASED",
    [Named, Amount],
    "`LogFactionChanged` (`0x50b8c0`): the faction, the delta."
);

/// Every family, for the sweep against the shipped templates.
#[cfg(test)]
pub(crate) const ALL_FAMILIES: &[Family] = &[
    TRADESKILL_LOG,
    FEEDPET_LOG,
    SPELLDISMISSPET,
    SPELLHAPPINESSDRAIN,
    OPEN_LOCK,
    DURABILITYDAMAGE_DEATH,
    FACTION_STANDING_INCREASED,
    FACTION_STANDING_DECREASED,
];

/// A line waiting on its names: the subject's, and the `Named` slot's when it is a lookup.
#[derive(Clone, Debug)]
pub(crate) struct PendingCombat {
    pub kind: ChatEventKind,
    pub family: Family,
    /// The subject is the player, which picks the `me` key.
    pub me: bool,
    /// The guid naming the `Subject` slot; 0 when it is already filled or unused.
    pub subject: u64,
    pub fills: Fills,
    pub named: Named,
    pub tries: u16,
}

/// What still has to be looked up before a line's [`Slot::Named`] can be filled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Named {
    /// The text is already in [`Fills::named`], or the family has no `Named` slot.
    Ready,
    /// An item entry, through the item cache.
    Item(u32),
    /// A unit guid, through the name cache.
    Unit(u64),
}

/// A line of `family` with nothing to look up.
pub(crate) fn ready(kind: ChatEventKind, family: Family, fills: Fills) -> PendingCombat {
    PendingCombat {
        kind,
        family,
        me: false,
        subject: 0,
        fills,
        named: Named::Ready,
        tries: 0,
    }
}

#[cfg(test)]
mod tests;
