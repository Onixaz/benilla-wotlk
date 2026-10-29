//! The companion feed and drain behind `GetNumCompanions` (`0x53c2a0`), `GetCompanionInfo`
//! (`0x53e2c0`), `GetCompanionCooldown` (`0x53e490`), `CallCompanion` (`0x53e5e0`),
//! `DismissCompanion` (`0x53e6b0`) and `SummonRandomCritter` (`0x53e7e0`): the stock pet paper
//! doll's Companions and Mounts tabs.
//!
//! A known spell past the registrar's `0x80`/`0x20` exits that summons into a slot-5
//! `SummonProperties` row or applies `MOUNTED` first joins a list (`0x5426a5`, [`CompanionKind`])
//! instead of the book. Each list sorts by its creature's cached name once no record is pending
//! (`0x53cec0`, `SStrSortUTF8I`); until then it keeps spell order.
//!
//! Events, in the reference's order:
//! - `COMPANION_UPDATE` (no type) whenever the lists re-sort (`0x53cec0`): the login once every
//!   record is cached, each learn and unlearn;
//! - `ERR_LEARN_COMPANION_S` with the creature's name, then `COMPANION_LEARNED`, on a live learn
//!   once its record is cached (`0x5427be`, `0x53ebc0`); `COMPANION_UNLEARNED` on an unlearn
//!   (`0x53fad0`);
//! - `COMPANION_UPDATE "CRITTER"` on a `UNIT_FIELD_CRITTER` change (`0x6cf6c0`), `"MOUNT"` on a
//!   `MOUNTED` aura's arrival or removal (`0x724820`, `0x71e930`).
//!
//! Not modelled: the vehicle arm of the active test (`0x802cf3`), and the learn's tutorials
//! (`0x33`, `0x34`).

use std::collections::BTreeSet;

use bevy::prelude::*;

use benilla_formats::{CompanionKind, SpellCatalog, SummonPropertiesCatalog, SPELL_AURA_MOUNTED};
use benilla_ui::script::{CompanionIntent, CompanionState, CompanionView, ScriptValue, UiScript};

use crate::names::{NameCache, NameKey};
use crate::net::{ClientCommand, NetCommands, ObjectStore, SelfPlayer};
use crate::spell::{CompanionEdges, Cooldowns, SummonSlots};
use crate::ui_action::{PlayerActions, Spells, UiError, UiErrorKeys};
use crate::ui_script::{gate, UiInput};
use crate::ui_unit::UnitFeed;
use crate::unit_auras::UnitAuras;

/// `SkillLine.dbc` "Companion Pets": `SummonRandomCritter`'s pool (`0x6e7bc4`).
const SKILL_LINE_COMPANIONS: u32 = 778;

pub(crate) struct UiCompanionPlugin;

impl Plugin for UiCompanionPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                // Before the cooldown events, whose `SPELL_UPDATE_COOLDOWN` makes the page re-read
                // `GetCompanionCooldown`, and after the book, which the learn line follows.
                feed_companions
                    .in_set(UnitFeed)
                    .after(crate::ui_spellbook::BookFeed)
                    .after(crate::ui_action::SlotIdentityFeed)
                    .before(crate::ui_action::CooldownEvents),
                drain_companions.after(UiInput),
                // After the VM ticks: a click's `SetCreature` lands during `UiInput`. Before the
                // display build that loads its model and the booth that draws it.
                feed_companion_booth
                    .after(UiInput)
                    .in_set(crate::portrait::CreatureBoothFeed)
                    .before(crate::entities::DisplayBuildSet),
            ),
        );
    }
}

/// What the feed last pushed and saw.
#[derive(Default)]
pub(crate) struct CompanionMemory {
    pushed: CompanionState,
    /// The critter guid and the `MOUNTED` auras' spells, for the typed updates.
    critter: Option<u64>,
    mount_auras: Vec<u32>,
    /// The gate's watches, and whether the last build waits on a name not yet answered.
    cooldown_epoch: gate::Watch,
    self_present: gate::Watch,
    names: gate::Watch,
    pending: bool,
}

/// A critter guid's entry bits, as `0x802dd8` takes them: bits 24..51, whatever the high word.
fn critter_entry(guid: u64) -> u32 {
    ((guid >> 24) & 0x0fff_ffff) as u32
}

/// The self player's companion facts: the critter out and the live auras.
#[derive(Clone, Copy, Default)]
struct SelfFacts<'a> {
    critter: Option<u64>,
    auras: Option<&'a UnitAuras>,
}

/// `Spell_C_IsActiveSpell` (`0x802cb0`) for a companion: a slot-5 summon is active while the
/// critter out is its creature (`0x802dcc`, the guid's entry bits); a mount while an aura of its
/// spell is up, or any `MOUNTED` aura riding the same creature (the walk's tail).
fn companion_active(
    spell_id: u32,
    d: &benilla_formats::SpellDisplay,
    kind: CompanionKind,
    catalog: &SpellCatalog,
    facts: SelfFacts,
) -> bool {
    match kind {
        CompanionKind::Critter => facts.critter.map(critter_entry) == Some(d.companion_creature()),
        CompanionKind::Mount => facts.auras.is_some_and(|auras| {
            auras.iter().any(|a| {
                a.spell_id == spell_id
                    || catalog.get(a.spell_id).is_some_and(|m| {
                        m.effect_apply_aura[0] == SPELL_AURA_MOUNTED
                            && m.effect_misc_value[0] == d.effect_misc_value[0]
                    })
            })
        }),
    }
}

/// The two lists from the known spells (module doc), with `pending` set while any creature record
/// is still asked.
#[allow(clippy::too_many_arguments)] // the feed's inputs, kept apart for the tests
fn build_lists(
    known: &BTreeSet<u32>,
    catalog: &SpellCatalog,
    summons: &SummonPropertiesCatalog,
    name_of: &dyn Fn(u32) -> Option<Option<String>>,
    facts: SelfFacts,
    cooldown_of: &dyn Fn(u32) -> Option<(i64, u32, bool)>,
) -> (CompanionState, bool) {
    let mut state = CompanionState::default();
    let mut pending = false;
    for &spell_id in known {
        let Some(d) = catalog.get(spell_id).filter(|d| d.in_name_cache()) else {
            continue;
        };
        let Some(kind) = d.companion_kind(summons) else {
            continue;
        };
        let creature_id = d.companion_creature();
        let name = match name_of(creature_id) {
            Some(answer) => answer,
            None => {
                pending = true;
                None
            }
        };
        let active = companion_active(spell_id, d, kind, catalog, facts);
        // `GetCompanionInfo`'s icon: the active one while active, when the row has one.
        let icon = if active {
            d.active_icon.clone().or_else(|| d.icon.clone())
        } else {
            d.icon.clone()
        };
        let view = CompanionView {
            creature_id,
            name,
            spell_id,
            icon,
            active,
            cooldown: cooldown_of(spell_id),
        };
        match kind {
            CompanionKind::Critter => state.critters.push(view),
            CompanionKind::Mount => state.mounts.push(view),
        }
    }
    if !pending {
        // `SortCompanions` (`0x53c7c0`): the names case-insensitively, as `SStrSortUTF8I` does.
        for list in [&mut state.critters, &mut state.mounts] {
            list.sort_by_cached_key(|c| {
                (
                    c.name.as_deref().unwrap_or_default().to_lowercase(),
                    c.spell_id,
                )
            });
        }
    }
    (state, pending)
}

/// The lists' identity, the part a re-sort changes: spells, creatures and names in order.
fn identity(state: &CompanionState) -> Vec<(u32, u32, Option<&str>)> {
    state
        .critters
        .iter()
        .chain(&state.mounts)
        .map(|c| (c.spell_id, c.creature_id, c.name.as_deref()))
        .collect()
}

#[allow(clippy::too_many_arguments)] // a Bevy system's full input set
fn feed_companions(
    script: Option<NonSendMut<UiScript>>,
    actions: Res<PlayerActions>,
    spells: Option<Res<Spells>>,
    summons: Res<SummonSlots>,
    names: Res<NameCache>,
    commands: Res<NetCommands>,
    cooldowns: Res<Cooldowns>,
    clock: Res<crate::ui_script::UiClock>,
    skill_lines: Option<Res<crate::ui_spellbook::SkillLines>>,
    self_q: Query<(Ref<ObjectStore>, Option<Ref<UnitAuras>>), With<SelfPlayer>>,
    mut edges: ResMut<CompanionEdges>,
    mut errors: ResMut<UiErrorKeys>,
    mut memory: Local<crate::ui_script::VmMemo<CompanionMemory>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let (memory, vm_reset) = memory.get_reset(&script);
    let Some(spells) = spells else {
        return;
    };
    let me_ref = self_q.iter().next();
    // The lists depend on these alone; `UiClock` is not one, as `ui_triple` is frame-stable per
    // arm and an expiry moves the store's `feed_epoch`.
    let cooldowns_moved = memory.cooldown_epoch.moved(cooldowns.feed_epoch());
    let presence_moved = memory.self_present.moved(u64::from(me_ref.is_some()));
    // The generation, not the change mark: an ask through `ResMut` marks the cache every frame.
    let names_moved = memory.names.moved(names.generation());
    // The frame a timer crosses zero, before the prune moves the epoch.
    let sweep = cooldowns.sweep_pending(clock.anchor);
    let self_changed = me_ref.as_ref().is_some_and(|(store, auras)| {
        store.is_changed() || auras.as_ref().is_some_and(|a| a.is_changed())
    });
    let inputs = [
        ("vm_reset", vm_reset),
        ("cooldowns", cooldowns_moved),
        ("presence", presence_moved),
        ("sweep", sweep),
        ("self", self_changed),
        ("actions", actions.is_changed()),
        ("spells", spells.is_changed()),
        ("summons", summons.is_changed()),
        ("names", names_moved),
        (
            "skill_lines",
            skill_lines.as_ref().is_some_and(|s| s.is_changed()),
        ),
        // A name still unanswered, and a learn or unlearn not yet spoken.
        ("pending", memory.pending),
        (
            "edges",
            !edges.learned.is_empty() || !edges.unlearned.is_empty(),
        ),
    ];
    gate::trace("feed_companions", &inputs);
    let gate = gate::Gate::new(inputs.iter().any(|&(_, open)| open));
    if gate.skip() {
        return;
    }
    let spells = &*spells;
    let me = me_ref.as_ref().map(|(s, a)| (&**s, a.as_deref()));
    let facts = SelfFacts {
        critter: me.and_then(|(s, _)| s.0.unit_critter()),
        auras: me.and_then(|(_, a)| a),
    };
    let name_of = |entry: u32| {
        names
            .answer(NameKey::Creature(entry), 0, &commands)
            .map(|n| n.map(str::to_string))
    };
    let (anchor, ui_now) = (clock.anchor, clock.ui_now);
    let cooldown_of = |spell_id: u32| {
        cooldowns
            .info(spell_id, 0, spells.catalog.get(spell_id), anchor)
            .ui_triple(anchor, ui_now)
    };
    let (mut fresh, pending) = build_lists(
        &actions.spells,
        &spells.catalog,
        &summons.0,
        &name_of,
        facts,
        &cooldown_of,
    );
    // `SummonRandomCritter`'s pool: the known spells of skill line 778, whatever their effect.
    let (race, class) = crate::spell::race_class(me.map(|(s, _)| s));
    fresh.random_pool = actions
        .spells
        .iter()
        .copied()
        .filter(|&id| {
            skill_lines
                .as_deref()
                .and_then(|s| s.catalog.spell_to_line(id, race, class))
                == Some(SKILL_LINE_COMPANIONS)
        })
        .collect();

    memory.pending = pending;
    // A live learn speaks once its record lands; a refused query drops it (`0x53ebc0`).
    let mut learned = 0;
    edges.learned.retain(|&spell_id| {
        let Some(d) = spells.catalog.get(spell_id) else {
            return false;
        };
        match name_of(d.companion_creature()) {
            None => true,
            Some(None) => false,
            Some(Some(name)) => {
                errors.0.push(UiError::s("ERR_LEARN_COMPANION_S", name));
                learned += 1;
                false
            }
        }
    });
    let unlearned = std::mem::take(&mut edges.unlearned).len();

    let resorted = !pending
        && (!fresh.critters.is_empty() || !fresh.mounts.is_empty())
        && identity(&fresh) != identity(&memory.pushed);
    if fresh != memory.pushed {
        gate.audit("feed_companions", "the lists");
        script.set_companions(fresh.clone());
        memory.pushed = fresh;
    }
    if resorted || learned > 0 || unlearned > 0 {
        debug!(
            "ui_companion: {} critter(s), {} mount(s) (learned {learned}, unlearned {unlearned})",
            memory.pushed.critters.len(),
            memory.pushed.mounts.len()
        );
    }
    if resorted {
        script.fire_event("COMPANION_UPDATE", vec![]);
    }
    for _ in 0..learned {
        script.fire_event("COMPANION_LEARNED", vec![]);
    }
    for _ in 0..unlearned {
        script.fire_event("COMPANION_UNLEARNED", vec![]);
    }

    if memory.critter != facts.critter {
        gate.audit("feed_companions", "the critter update");
        memory.critter = facts.critter;
        script.fire_event("COMPANION_UPDATE", vec![ScriptValue::Str("CRITTER".into())]);
    }
    let mount_auras: Vec<u32> = facts
        .auras
        .map(|auras| {
            auras
                .iter()
                .filter(|a| {
                    spells
                        .catalog
                        .get(a.spell_id)
                        .is_some_and(|d| d.effect_apply_aura.contains(&SPELL_AURA_MOUNTED))
                })
                .map(|a| a.spell_id)
                .collect()
        })
        .unwrap_or_default();
    if memory.mount_auras != mount_auras {
        gate.audit("feed_companions", "the mount update");
        memory.mount_auras = mount_auras;
        script.fire_event("COMPANION_UPDATE", vec![ScriptValue::Str("MOUNT".into())]);
    }
}

/// `Spell_C_CancelAura` (`0x802f80`) on our own player for one companion: nothing while charmed;
/// a slot-5 summon whose creature is out sends `CMSG_DISMISS_CRITTER` with the critter's guid
/// (`0x80303d`); an aura spell cancels its aura, a mount ridden through another spell of the same
/// creature that one (`0x8030bc`).
fn dismiss(
    spell_id: u32,
    catalog: &SpellCatalog,
    summons: &SummonPropertiesCatalog,
    store: &ObjectStore,
    auras: Option<&UnitAuras>,
) -> Vec<ClientCommand> {
    let Some(d) = catalog.get(spell_id) else {
        return Vec::new();
    };
    if store.0.unit_charmed_by().is_some() {
        return Vec::new();
    }
    let mut sends = Vec::new();
    if d.companion_kind(summons) == Some(CompanionKind::Critter) {
        if let Some(guid) = store.0.unit_critter() {
            if critter_entry(guid) == d.companion_creature() {
                sends.push(ClientCommand::DismissCritter { guid });
            }
        }
    }
    if d.effect_apply_aura.iter().all(|&a| a == 0) {
        return sends;
    }
    let ridden = (store.0.unit_mount_display_id() > 0
        && d.effect_apply_aura.contains(&SPELL_AURA_MOUNTED))
    .then(|| {
        auras?.iter().find_map(|a| {
            let m = catalog.get(a.spell_id)?;
            (m.effect_apply_aura[0] == SPELL_AURA_MOUNTED
                && m.effect_misc_value[0] == d.effect_misc_value[0]
                && a.spell_id != spell_id)
                .then_some(a.spell_id)
        })
    })
    .flatten();
    sends.push(ClientCommand::CancelAura {
        spell_id: ridden.unwrap_or(spell_id),
    });
    sends
}

/// The dismissals `DismissCompanion` queued.
fn drain_companions(
    script: Option<NonSendMut<UiScript>>,
    spells: Option<Res<Spells>>,
    summons: Res<SummonSlots>,
    commands: Res<NetCommands>,
    self_q: Query<(&ObjectStore, Option<&UnitAuras>), With<SelfPlayer>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let intents = script.take_companion_intents();
    if intents.is_empty() {
        return;
    }
    let (Some(spells), Some((store, auras))) = (spells.as_deref(), self_q.iter().next()) else {
        return;
    };
    for CompanionIntent::Dismiss(spell_id) in intents {
        for send in dismiss(spell_id, &spells.catalog, &summons.0, store, auras) {
            debug!("ui_companion: dismiss {spell_id} — {send:?}");
            let _ = commands.0.send(send);
        }
    }
}

/// Point the preview booth at the pane's `SetCreature` creature by its cached record's display
/// (`0x597960`, `CreatureStats::m_displayID[0]`), asking for a record not yet cached. The pane's
/// rotate buttons own the yaw.
fn feed_companion_booth(
    script: Option<NonSend<UiScript>>,
    names: Res<NameCache>,
    commands: Res<NetCommands>,
    mut booth: ResMut<crate::portrait::CompanionBooth>,
) {
    let Some(script) = script else {
        return;
    };
    let pane = script.model_pane("CompanionModelFrame");
    let yaw = pane.as_ref().map_or(0.61, |m| m.facing);
    let display = pane.and_then(|m| m.creature).and_then(|entry| {
        let record = names.creature_record(entry);
        if record.is_none() {
            names.resolve_creature(entry, 0, &commands);
        }
        record.map(|r| r.display_id).filter(|&d| d != 0)
    });
    if booth.yaw != yaw {
        booth.yaw = yaw;
    }
    if booth.display_id != display {
        booth.display_id = display;
    }
}

#[cfg(test)]
#[path = "ui_companion_tests.rs"]
mod tests;
