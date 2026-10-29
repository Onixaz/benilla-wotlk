//! The unit snapshot and event feed: each frame, ahead of the VM's tick, live ECS state becomes a
//! [`UnitState`] per token for the engine-free `Unit*` bindings, plus the unit events; the Lua API
//! never reaches into the ECS.

use std::collections::HashMap;

use bevy::prelude::*;

use benilla_formats::ChrClasses;
use benilla_protocol::messages::ObjectType;
use benilla_ui::script::{
    parse_unit_token, power_token, ScriptValue, UiScript, UnitBase, UnitState, UnitTokenParse,
    WornDisplay,
};

use crate::creature_type::CreatureTypeSources;
use crate::names::NameCache;
use crate::net::{
    FieldChanged, FieldEdges, Guid, NetCommands, NetHandlerApp, ObjectStore, Reputations,
    SelfPlayer,
};
use crate::target::{ring_reaction, Factions, Selection};
use crate::ui_script::{gate, UiInput};

mod held;

/// The unit-feed pass, gated so none of its login one-shots or per-VM memos runs before the in-game
/// interface exists; the demo override orders itself after it.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct UnitFeed;

/// The system firing `PLAYER_ENTERING_WORLD`; a feed its stock handlers read runs before it.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct EnterWorldFeed;

/// One `UNIT_COMBAT` over a unit, the portrait hit indicator's feed: the unit-combat signal
/// (`0x5132c0`) fires `(token, action, descriptor, amount, school)` once per token naming the guid
/// (`0x60bb70`), with no self-suppression and no CVar gate. The melee victim event waits for the
/// impact keyframe.
#[derive(Message, Clone, Copy)]
pub(crate) struct UnitCombatFeedback {
    pub(crate) guid: u64,
    /// `arg2`, the action word (`WOUND`, `MISS`, `DODGE`, `PARRY`, `BLOCK`, `HEAL`, …).
    pub(crate) action: &'static str,
    /// `arg3`, the descriptor: `CRITICAL`/`CRUSHING`/`GLANCING`/`ABSORB`/`BLOCK`/`RESIST`, or `""`.
    pub(crate) flags: &'static str,
    /// `arg4`, the amount; 0 for a word alone.
    pub(crate) amount: i32,
    /// `arg5`, the school mask.
    pub(crate) school: u32,
}

/// One `COMBAT_TEXT_UPDATE` (the combat-text send, `0x74d460`-`0x74d550`): the type word, then the
/// fire's own `%s`/`%d` arguments. The combat log fires it only for the combat-text unit, the
/// player (`0x74d430`).
#[derive(Message, Clone)]
pub(crate) struct CombatTextEvent {
    pub(crate) message_type: &'static str,
    pub(crate) args: Vec<ScriptValue>,
}

/// `PLAYER_LEAVING_WORLD` on a cross-map worldport. The reference fires event `0x111` at one site,
/// gated only by the latch, from the local player's destructor (this system), the shutdown tail
/// (ours is `shutdown_ui_state`) and the local player's DESTROY or OUT_OF_RANGE; a same-map
/// teleport destroys nothing.
///
/// Deviation: fires with our descriptor present, so `UnitExists("player")` answers true where the
/// reference's misses, because matching it breaks `UnitExists` for no stock reader.
pub(crate) fn fire_leaving_world_on_worldport(
    script: Option<NonSendMut<UiScript>>,
    mut sweep: crate::ui_script::LeaveWorldSweep,
    mut ports: MessageReader<crate::net::WorldportMessage>,
) {
    // `needs_ack` false is the initial-login map, an arrival; the whole iterator is read so no
    // message carries over.
    let leaving = ports.read().filter(|w| w.needs_ack).count() > 0;
    if !leaving {
        return;
    }
    // The world latch keeps this producer and the shutdown tail from both claiming one departure;
    // spent even with no VM, as the reference clears it, ahead of the fire.
    if !sweep.run() {
        return;
    }
    let Some(mut script) = script else {
        return;
    };
    script.fire_event("PLAYER_LEAVING_WORLD", Vec::new());
}

/// The feed's change-tracking memory: what we last told the VM.
#[derive(Resource, Default)]
struct UnitFeedState {
    /// What we last told the VM, dying with it, so a `/reload` re-fires everything as a login does.
    vm: crate::ui_script::VmMemo<UnitFeedMemo>,
}

/// The per-VM half of [`UnitFeedState`]: the event-trigger diffs.
#[derive(Default)]
struct UnitFeedMemo {
    /// The lazy caches' landing counters: their per-frame `&mut` misses would trip `is_changed`.
    names_generation: gate::Watch,
    guild_generation: gate::Watch,
    /// The VM's focus guid, which `FocusUnit` moves with no other input changing.
    focus_guid: gate::Watch,
    /// Whether `PLAYER_ENTERING_WORLD` has fired for this world entry.
    entered_world: bool,
    /// Per token, the last snapshot pushed.
    last: HashMap<String, UnitState>,
    target_guid: Option<u64>,
    last_xp: Option<(u32, u32)>,
    /// `(rest state, pool, PLAYER_FLAGS)`, pushed as one so no binding reads it half-updated.
    last_rest: Option<(u8, u32, u32)>,
    /// Our last level; the first sighting is the login descriptor, not a ding.
    last_level: Option<u32>,
    /// Our last in-combat flag; first sight fires only when already in combat.
    in_combat: Option<bool>,
    /// Our last PvP-preference bit; the first descriptor is silent, as the reference diffs bits.
    pvp_desired: Option<bool>,
    /// `(HIDE_HELM, HIDE_CLOAK)`, pushed on the edge only: the Options setter flips the VM's
    /// belief ahead of the server, and a per-frame push would snap it back.
    worn_hidden: Option<(bool, bool)>,
    /// `PLAYER_FIELD_BYTES` byte 2 (`GetActionBarToggles`); no field watch, no event.
    action_bar_toggles: Option<u8>,
}

/// Adds the per-frame unit feed; the `Unit*` bindings live in `benilla-ui`.
pub(crate) struct UiUnitPlugin;

impl Plugin for UiUnitPlugin {
    fn build(&self, app: &mut App) {
        app.configure_sets(
            Update,
            UnitFeed
                .in_set(crate::ui_script::UiFeed)
                // The whole set: a login one-shot or a per-VM memo spent on the boot VM is lost.
                .run_if(crate::ui_script::ingame_ui_up),
        )
        .init_resource::<UnitFeedState>()
        // A UI-only harness has no sound or net stack, and a missing resource or message is a
        // system-validation panic; declaring twice is idempotent.
        .init_resource::<crate::sound::MessageSounds>()
        .add_message::<UnitCombatFeedback>()
        .add_message::<CombatTextEvent>()
        .add_message::<crate::net::WorldportMessage>()
        .add_message::<crate::net::FieldChanged>()
        .init_resource::<crate::ui_script::LeavingWorldArmed>()
        .add_message::<crate::ui_script::WorldLeaveSweepMessage>()
        .add_systems(
            Update,
            (
                // First, so a worldport's leaving edge precedes the entering edge `feed_units`
                // raises for the same port.
                fire_leaving_world_on_worldport,
                feed_units.in_set(EnterWorldFeed),
                // After the aura feed, whose resolver inputs it measures.
                feed_unit_reach.after(crate::ui_aura::AuraEvents),
                // After the same feed, whose guids it covers.
                held::feed_chain_units.after(crate::ui_aura::AuraEvents),
                feed_player_control,
                feed_farsight_focus,
                melee_unit_combat,
                fire_unit_combat,
                fire_combat_text,
            )
                .chain()
                .in_set(UnitFeed),
        )
        // After the unit snapshots and the resolver's guids, which its handlers read.
        .add_systems(
            Update,
            feed_combo_points
                .in_set(UnitFeed)
                .after(feed_units)
                .after(crate::ui_aura::AuraEvents),
        )
        .add_systems(Update, drain_pvp_toggles.after(UiInput))
        .net_handler(
            benilla_protocol::SessionEventKind::PowerUpdate,
            on_power_update,
        )
        .add_systems(Update, drain_worn_display_toggles.after(UiInput))
        .add_systems(Update, drain_action_bar_toggles.after(UiInput))
        .add_systems(Update, feed_default_language.in_set(UnitFeed))
        .add_systems(Update, feed_known_languages.in_set(UnitFeed))
        .add_systems(Update, feed_option_state.in_set(UnitFeed))
        // `load_exhaustion_rows` pushes into the VM, so it runs per VM; the rest are one-shots.
        .add_systems(
            Update,
            load_exhaustion_rows.in_set(crate::ui_script::UiFeed),
        )
        .add_systems(PostStartup, (load_default_languages, load_languages));
    }
}

/// The race to default-chat-language join; absent when the tables do not load, and
/// `GetDefaultLanguage()` then answers the reference's zero-value shape.
#[derive(Resource)]
pub(crate) struct DefaultLanguagesRes(pub(crate) benilla_formats::DefaultLanguages);

/// `Languages.dbc` in row order, the walk behind `GetNumLanguages`/`GetLanguageByIndex`.
#[derive(Resource)]
pub(crate) struct LanguagesRes(pub(crate) benilla_formats::Languages);

fn load_languages(mut commands: Commands, assets: Option<Res<benilla_assets::WorldAssets>>) {
    let Some(assets) = assets else { return };
    let loaded = {
        use benilla_assets::LockRecover;
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_languages(&mut chain)
    };
    match loaded {
        Ok(langs) => {
            info!("ui_unit: {} Languages.dbc rows", langs.len());
            commands.insert_resource(LanguagesRes(langs));
        }
        Err(e) => warn!("ui_unit: Languages.dbc unavailable — {e:#}"),
    }
}

/// The languages this character knows, in `Languages.dbc` order: one a known spell declares
/// (`Effect_1 == 39`, spell to language, never the reverse) whose skill line is present in
/// `PLAYER_SKILL_INFO`, whatever its value. Two spells on one language both count here; the
/// reference keeps the later learn, invisible on shipped data.
pub(crate) fn known_languages(
    known: impl IntoIterator<Item = u32>,
    spells: &benilla_formats::SpellCatalog,
    skill_lines: Option<&benilla_formats::SkillLineCatalog>,
    (race, class): (u8, u8),
    has_skill_line: impl Fn(u32) -> bool,
    languages: &benilla_formats::Languages,
) -> Vec<String> {
    let mut declared: std::collections::HashMap<u32, Vec<u32>> = Default::default();
    for spell in known {
        if let Some(lang) = spells.declared_language(spell) {
            declared.entry(lang).or_default().push(spell);
        }
    }
    languages
        .names(0)
        .filter(|(id, _)| {
            declared.get(id).is_some_and(|spells| {
                spells.iter().any(|&spell| {
                    skill_lines
                        .and_then(|sl| sl.spell_to_line(spell, race, class))
                        .is_some_and(&has_skill_line)
                })
            })
        })
        .map(|(_, name)| name.to_string())
        .collect()
}

fn feed_known_languages(
    script: Option<NonSendMut<UiScript>>,
    actions: Option<Res<crate::ui_action::PlayerActions>>,
    spells: Option<Res<crate::ui_action::Spells>>,
    skill_lines: Option<Res<crate::ui_spellbook::SkillLines>>,
    languages: Option<Res<LanguagesRes>>,
    self_q: Query<Ref<ObjectStore>, With<SelfPlayer>>,
    mut pushed: Local<crate::ui_script::VmMemo<Option<Vec<String>>>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let (Some(actions), Some(spells), Some(languages)) = (actions, spells, languages) else {
        return;
    };
    let pushed = pushed.get(&script);
    let store = self_q.iter().next();
    // With no input moved since this VM's push, a rebuild could only reproduce the memo.
    let inputs_moved = store.as_ref().is_some_and(|s| s.is_changed())
        || actions.is_changed()
        || spells.is_changed()
        || languages.is_changed()
        || skill_lines.as_ref().is_some_and(|l| l.is_changed());
    if pushed.is_some() && !inputs_moved {
        return;
    }
    let store: Option<&ObjectStore> = store.as_deref();
    let has_skill_line = |line: u32| {
        store.is_some_and(|s| {
            (0..benilla_protocol::messages::PLAYER_SKILL_SLOTS)
                .filter_map(|i| s.0.player_skill(i))
                .any(|slot| u32::from(slot.skill_id) == line)
        })
    };
    let names = known_languages(
        actions.spells.iter().copied(),
        &spells.catalog,
        skill_lines.as_deref().map(|s| &*s.catalog),
        crate::spell::race_class(store),
        has_skill_line,
        &languages.0,
    );
    if pushed.as_ref() != Some(&names) {
        script.set_known_languages(names.clone());
        *pushed = Some(names);
    }
}

fn load_default_languages(
    mut commands: Commands,
    assets: Option<Res<benilla_assets::WorldAssets>>,
) {
    let Some(assets) = assets else { return };
    let loaded = {
        use benilla_assets::LockRecover;
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_default_languages(&mut chain)
    };
    match loaded {
        Ok(langs) => {
            info!("ui_unit: {} race → default-language rows", langs.len());
            commands.insert_resource(DefaultLanguagesRes(langs));
        }
        // Not fatal: the binding answers the no-table case.
        Err(e) => warn!("ui_unit: default languages unavailable — {e:#}"),
    }
}

/// Seed a new VM's default language from the roster row, before the avatar exists: the load burst
/// reads it, and so does `ChatFrame.lua:1276` at a `PLAYER_ENTERING_WORLD` that [`feed_units`]
/// fires unordered against [`feed_default_language`].
pub(crate) fn seed_default_language(world: &mut World, script: &mut UiScript) {
    let (Some(langs), Some(roster)) = (
        world.get_resource::<DefaultLanguagesRes>(),
        world.get_resource::<crate::char_select::Roster>(),
    ) else {
        return;
    };
    let Some(row) = roster.pending_row() else {
        return;
    };
    script.set_default_language(langs.0.name(u32::from(row.race), 0).map(str::to_string));
}

/// Seed a new VM with `Languages.dbc`, the static table `SendChatMessage` resolves its language
/// name against; locale column 0, as [`feed_default_language`] reads.
pub(crate) fn seed_language_table(world: &World, script: &mut UiScript) {
    let Some(langs) = world.get_resource::<LanguagesRes>() else {
        return;
    };
    script.set_language_table(
        langs
            .0
            .names(0)
            .map(|(id, name)| (id, name.to_string()))
            .collect(),
    );
}

/// Push `GetDefaultLanguage()`'s string on a change of race; `None` is the reference's zero value.
/// Locale column 0: only enUS is populated in the 1.12 data.
fn feed_default_language(
    script: Option<NonSendMut<UiScript>>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    langs: Option<Res<DefaultLanguagesRes>>,
    mut pushed: Local<crate::ui_script::VmMemo<Option<Option<String>>>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let pushed = pushed.get(&script);
    let name = self_q
        .iter()
        .next()
        .and_then(|store| store.0.unit_race())
        .zip(langs.as_ref())
        .and_then(|(race, langs)| langs.0.name(u32::from(race), 0))
        .map(str::to_string);
    if pushed.as_ref() != Some(&name) {
        script.set_default_language(name.clone());
        *pushed = Some(name);
    }
}

/// Seed each VM's `Exhaustion.dbc` table; a failed load keeps the shipped-table fallback.
fn load_exhaustion_rows(
    script: Option<NonSendMut<UiScript>>,
    assets: Option<Res<benilla_assets::WorldAssets>>,
    mut seeded: Local<crate::ui_script::VmMemo<bool>>,
) {
    let (Some(mut script), Some(assets)) = (script, assets) else {
        return;
    };
    if !seeded.claim(&script) {
        return;
    }
    let loaded = {
        use benilla_assets::LockRecover;
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_exhaustion(&mut chain)
    };
    match loaded {
        Ok(rows) => {
            info!("ui_unit: {} Exhaustion.dbc rest states", rows.len());
            script.set_exhaustion_rows(
                rows.into_iter()
                    .map(|r| (r.id as u8, r.name, f64::from(r.factor)))
                    .collect(),
            );
        }
        Err(e) => error!("ui_unit: Exhaustion.dbc failed — shipped-table fallback holds: {e:#}"),
    }
}

/// Melee swing to the `UNIT_COMBAT` action and descriptor (`0x5132c0`).
fn melee_feedback(hit_info: u32, victim_state: u32, damage: u32) -> (&'static str, &'static str) {
    use benilla_protocol::messages::hit_info::{
        BLOCK, CRIT, CRUSHING, FULL_ABSORB, FULL_RESIST, GLANCING,
    };
    match victim_state {
        2 => ("DODGE", ""),
        3 => ("PARRY", ""),
        5 => ("BLOCK", ""),
        6 => ("EVADE", ""),
        7 => ("IMMUNE", ""),
        8 => ("DEFLECT", ""),
        // 0 UNAFFECTED / 1 NORMAL / 4 INTERRUPT: WOUND, descriptor by the amount-sign key.
        _ => {
            if damage > 0 {
                if hit_info & CRIT != 0 {
                    ("WOUND", "CRITICAL")
                } else if hit_info & GLANCING != 0 {
                    ("WOUND", "GLANCING")
                } else if hit_info & CRUSHING != 0 {
                    ("WOUND", "CRUSHING")
                } else {
                    ("WOUND", "")
                }
            } else if hit_info & FULL_ABSORB != 0 {
                ("WOUND", "ABSORB")
            } else if hit_info & BLOCK != 0 {
                ("WOUND", "BLOCK") // a full block the parse did not already make state 5
            } else if hit_info & FULL_RESIST != 0 {
                ("WOUND", "RESIST")
            } else {
                ("MISS", "")
            }
        }
    }
}

/// The melee `UNIT_COMBAT` producer, on the swing's impact keyframe, `text_only` flushes included;
/// the center combat text is not here, as the reference fires it at packet parse.
fn melee_unit_combat(
    mut impacts: MessageReader<crate::creature_anim::SwingImpact>,
    guids: Query<&crate::net::Guid>,
    mut out: MessageWriter<UnitCombatFeedback>,
) {
    for crate::creature_anim::SwingImpact { swing: s, .. } in impacts.read() {
        let Some(victim) = s.victim.and_then(|v| guids.get(v).ok()) else {
            continue;
        };
        let (action, flags) = melee_feedback(s.hit_info, s.victim_state, s.damage);
        out.write(UnitCombatFeedback {
            guid: victim.0,
            action,
            flags,
            amount: s.damage as i32,
            school: 1, // the swing's first sub-damage school, physical for a weapon
        });
    }
}

/// Drain [`CombatTextEvent`] into `COMBAT_TEXT_UPDATE(messageType, data, extra)`.
fn fire_combat_text(
    script: Option<NonSendMut<UiScript>>,
    mut events: MessageReader<CombatTextEvent>,
) {
    let Some(mut script) = script else {
        return;
    };
    for ev in events.read() {
        let mut args = Vec::with_capacity(ev.args.len() + 1);
        args.push(ScriptValue::Str(ev.message_type.to_string()));
        args.extend(ev.args.iter().cloned());
        script.fire_event("COMBAT_TEXT_UPDATE", args);
    }
}

/// Drain [`UnitCombatFeedback`] into `UNIT_COMBAT`, once per token naming the guid (`0x60bb70`).
fn fire_unit_combat(
    script: Option<NonSendMut<UiScript>>,
    mut events: MessageReader<UnitCombatFeedback>,
) {
    let Some(mut script) = script else {
        return;
    };
    for ev in events.read() {
        for token in script.unit_tokens(ev.guid) {
            script.fire_event(
                "UNIT_COMBAT",
                vec![
                    ScriptValue::Str(token),
                    ScriptValue::Str(ev.action.to_string()),
                    ScriptValue::Str(ev.flags.to_string()),
                    ScriptValue::Int(i64::from(ev.amount)),
                    ScriptValue::Int(i64::from(ev.school)),
                ],
            );
        }
    }
}

/// The race id to `UnitRace`'s display name and `raceFile` token (`"Scourge"`, `"NightElf"`); the
/// display name is also what `$R`/`$r` expand to.
pub(crate) fn race_names(race: u8) -> Option<(&'static str, &'static str)> {
    Some(match race {
        1 => ("Human", "Human"),
        2 => ("Orc", "Orc"),
        3 => ("Dwarf", "Dwarf"),
        4 => ("Night Elf", "NightElf"),
        5 => ("Undead", "Scourge"),
        6 => ("Tauren", "Tauren"),
        7 => ("Gnome", "Gnome"),
        8 => ("Troll", "Troll"),
        10 => ("Blood Elf", "BloodElf"),
        11 => ("Draenei", "Draenei"),
        _ => return None,
    })
}

/// The class id to `UnitClass`'s display name and uppercase `classFileName`.
pub(crate) fn class_names(class: u8) -> Option<(&'static str, &'static str)> {
    Some(match class {
        1 => ("Warrior", "WARRIOR"),
        2 => ("Paladin", "PALADIN"),
        3 => ("Hunter", "HUNTER"),
        4 => ("Rogue", "ROGUE"),
        5 => ("Priest", "PRIEST"),
        6 => ("Death Knight", "DEATHKNIGHT"),
        7 => ("Shaman", "SHAMAN"),
        8 => ("Mage", "MAGE"),
        9 => ("Warlock", "WARLOCK"),
        11 => ("Druid", "DRUID"),
        _ => return None,
    })
}

/// A playable race's fixed side, for where no faction template is at hand, as at world entry,
/// where addons concatenate `UnitFactionGroup("player")` at file scope; [`faction_group`] reads
/// the live template.
pub(crate) fn race_faction_group(race: u8) -> Option<&'static str> {
    // The ten playable 3.3.5a races' sides, as `ChrRaces.dbc` seats them.
    match race {
        1 | 3 | 4 | 7 | 11 => Some("Alliance"),
        2 | 5 | 6 | 8 | 10 => Some("Horde"),
        _ => None,
    }
}

/// A unit's PvP team digit, the reference's `0` Horde, `1` Alliance, `-1` no side, from the race
/// and never the live template: `ChrRaces.dbc` field 2 to `FactionTemplate.dbc` field 3's group
/// mask, `& 4` Horde, else `& 2` Alliance, so a template-35 GM keeps his rank title. Read by
/// `GetPVPRankInfo`, `UnitPVPName` and the scoreboard. A frozen copy of the shipped eleven rows;
/// race 9 (Goblin) answers Alliance.
pub(crate) fn race_pvp_team(race: u8) -> i8 {
    match race {
        // Group mask 3, Player|Alliance: `& 4` clear, `& 2` set.
        1 | 3 | 4 | 7 | 9 | 11 => 1,
        // Group mask 5, Player|Horde: `& 4` set, tested first.
        2 | 5 | 6 | 8 | 10 => 0,
        // No `ChrRaces` row: the engine's bounds-failure `-1`, which names no GlobalString.
        _ => -1,
    }
}

/// Resolve a UnitPopup token to the other player's guid it names: `"target"` when it is a player,
/// `"partyN"` through the roster; `"player"` and anything unresolved answer `None`.
pub(crate) fn player_token_guid(
    token: &str,
    selection: &Selection,
    group: &crate::ui_party::GroupState,
) -> Option<u64> {
    match token {
        "target" => selection
            .guid
            .filter(|g| benilla_protocol::guid::is_player(*g)),
        "player" => None,
        tok => tok
            .strip_prefix("party")
            .and_then(|n| n.parse::<usize>().ok())
            .and_then(|n| n.checked_sub(1))
            .and_then(|n| group.party_slots().nth(n))
            .map(|m| m.guid),
    }
}

/// The one unit-token resolver, the reference's: case-insensitive compares, then the object
/// manager; a caller wanting a type tests the resolved unit. [`Selection`] is a parameter because
/// [`crate::target::SelectCommit`] holds it as `ResMut`.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct UnitTokens<'w, 's> {
    /// `Option`, like those below: a UI-only harness lacks their plugins; a bare `Res` panics.
    index: Option<Res<'w, crate::net::GuidIndex>>,
    pet: Option<Res<'w, crate::ui_pet::PetBar>>,
    hovered: Option<Res<'w, crate::target::Hovered>>,
    hovered_go: Option<Res<'w, crate::target::HoveredObject>>,
    interact: Option<Res<'w, crate::ui_session::InteractNpc>>,
    quest: Option<Res<'w, crate::ui_quest::QuestGiver>>,
    group: Res<'w, crate::ui_party::GroupState>,
    pub(crate) stores: Query<'w, 's, &'static ObjectStore>,
    me: Query<'w, 's, (Entity, &'static Guid), With<SelfPlayer>>,
}

impl UnitTokens<'_, '_> {
    fn me(&self) -> Option<(Entity, u64)> {
        self.me.iter().next().map(|(e, g)| (e, g.0))
    }

    /// The guid lookup, `pub(crate)` for `TargetLastEnemy`, which starts from a guid.
    pub(crate) fn held(&self, guid: u64) -> Option<(Entity, u64)> {
        Some((*self.index.as_ref()?.0.get(&guid)?, guid))
    }

    /// Resolve `token` as the reference does: its base, then a hop for each `target` after it. The
    /// text is read by [`parse_unit_token`], which the VM's resolver shares, so both take the same
    /// tokens; a token that names nobody, or a base or hop the object manager does not hold, is
    /// `None`.
    pub(crate) fn resolve(&self, token: &str, selection: &Selection) -> Option<(Entity, u64)> {
        let UnitTokenParse::Unit { base, hops } = parse_unit_token(token) else {
            return None;
        };
        let mut unit = self.base(base, selection)?;
        for _ in 0..hops {
            unit = self.target_of(unit.0)?;
        }
        Some(unit)
    }

    /// The held unit a token's base names, before any `target` hop.
    fn base(&self, base: UnitBase, selection: &Selection) -> Option<(Entity, u64)> {
        match base {
            UnitBase::Player => self.me(),
            UnitBase::Target => selection.target.zip(selection.guid),
            // The same pick `ui_tooltip` pushes `"mouseover"` from.
            UnitBase::Mouseover => self
                .hovered
                .as_ref()?
                .mouseover(&self.hovered_go.as_deref().copied().unwrap_or_default()),
            // Off the pet bar's cached guid, as the `"pet"` snapshot reads it.
            UnitBase::Pet => {
                let guid = self.pet.as_ref()?.spells.pet_guid;
                (guid != 0).then(|| self.held(guid)).flatten()
            }
            UnitBase::PartyPet(row) => self
                .party_member(row)
                .and_then(|member| self.pet_of(member, false)),
            UnitBase::RaidPet(row) => self
                .raid_member(row)
                .and_then(|member| self.pet_of(member, true)),
            UnitBase::Raid(row) => self.raid_member(row).and_then(|guid| self.held(guid)),
            UnitBase::Party(row) => self.party_member(row).and_then(|guid| self.held(guid)),
            // The interaction NPC's guid.
            UnitBase::Npc => self.held(self.interact.as_ref()?.1?),
            UnitBase::QuestNpc => self.held(self.quest.as_ref()?.quest_npc()?),
            // 12340's bases the app has no unit for yet (#90).
            UnitBase::Vehicle
            | UnitBase::Focus
            | UnitBase::Boss(_)
            | UnitBase::Arena(_)
            | UnitBase::ArenaPet(_)
            | UnitBase::Commentator(_) => None,
        }
    }

    /// One `target` hop: the held unit's `UNIT_FIELD_TARGET`, held in turn. The lookup takes the
    /// unit typemask, so a hop off any other object, or off a unit with no target, is nobody.
    fn target_of(&self, unit: Entity) -> Option<(Entity, u64)> {
        let store = self.stores.get(unit).ok().filter(|s| s.is_unit())?;
        self.held(store.0.unit_target()?)
    }

    /// The guid in 0-based party slot `row`, the one `partyN` and `partypetN` share.
    fn party_member(&self, row: u32) -> Option<u64> {
        let slot = usize::try_from(row).ok()?;
        self.group.party_slots().nth(slot).map(|m| m.guid)
    }

    /// The guid on 0-based raid row `row`, the one `raidN` and `GetRaidRosterInfo` share.
    fn raid_member(&self, row: u32) -> Option<u64> {
        let index = usize::try_from(row).ok()?.checked_add(1)?;
        crate::ui_party::raid_row_guid(&self.group, self.me().map(|(_, g)| g), index)
    }

    /// A group member's pet, held: `CHARM`, else `SUMMON`, off the member's descriptor while it is
    /// held, else off the roster record's pet guid (`partypetN`, `raidpetN`). A pet the object
    /// manager does not hold names no unit here, as any unheld unit.
    fn pet_of(&self, member: u64, raid: bool) -> Option<(Entity, u64)> {
        let live = self
            .held(member)
            .and_then(|(entity, _)| self.stores.get(entity).ok())
            .map(|store| &store.0);
        let pet = if raid {
            self.group.raid_pet_guid(member, live)
        } else {
            self.group.party_pet_guid(member, live)
        };
        self.held(pet?)
    }
}

/// Squared distance as the reference sums it: `f32` widened to `f64`, `(dz² + dx²) + dy²` (shared
/// by `CanInspect` and `CheckInteractDistance`). Bevy's axes change it by at most a last ulp, which
/// matters only on a threshold.
fn dist_sq(q: Vec3, p: Vec3) -> f64 {
    let dx = f64::from(q.x) - f64::from(p.x);
    let dy = f64::from(q.y) - f64::from(p.y);
    let dz = f64::from(q.z) - f64::from(p.z);
    (dz * dz + dx * dx) + dy * dy
}

/// `PLAYER_CONTROL_LOST`/`GAINED` on a change of the flag `SMSG_CLIENT_CONTROL_UPDATE` writes
/// (`0x520fe0`); the boot value, and a fresh VM's memo, is in control. A world entry sets the flag
/// with no event (`0x513880`).
fn feed_player_control(
    script: Option<NonSendMut<UiScript>>,
    player: Option<Res<crate::player::Player>>,
    mut memo: Local<crate::ui_script::VmMemo<(bool, u32)>>,
) {
    let (Some(mut script), Some(player)) = (script, player) else {
        return;
    };
    let (lost, inits) = memo.get(&script);
    if *inits != player.control_inits {
        *inits = player.control_inits;
        if !player.control_lost {
            *lost = false;
            script.set_player_control(true);
        }
    }
    if *lost != player.control_lost {
        *lost = player.control_lost;
        // `HasFullControl`'s flag rides the same edge.
        script.set_player_control(!player.control_lost);
        let event = if player.control_lost {
            "PLAYER_CONTROL_LOST"
        } else {
            "PLAYER_CONTROL_GAINED"
        };
        script.fire_event(event, vec![]);
    }
}

/// `PLAYER_FARSIGHT_FOCUS_CHANGED`: the `PLAYER_FARSIGHT` field callback fires it on every change,
/// whether or not the new guid resolves, so this diffs the field, never the pose.
fn feed_farsight_focus(
    script: Option<NonSendMut<UiScript>>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    mut focus: Local<crate::ui_script::VmMemo<Option<u64>>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let Some(store) = self_q.iter().next() else {
        return;
    };
    let anchor = store.0.player_farsight();
    let focus = focus.get(&script);
    if *focus != anchor {
        *focus = anchor;
        script.fire_event("PLAYER_FARSIGHT_FOCUS_CHANGED", vec![]);
    }
}

/// Feed the unit reach map: per held unit a token can name, its squared distance and whether it
/// passes inspect's two other refusals, a non-player and an attackable one (vmangos
/// `MiscHandler.cpp:945-956`; that the client checks them is inferred, being partly undecoded). The
/// guids are the resolver's own inputs, which the aura feed pushes each frame
/// ([`UiScript::held_unit_guids`], so this runs after it): the bases and each unit a `target` chain
/// reaches, and the VM resolves a verb's token to one of them or to a guid with no entry. Ungated:
/// distances move every frame, and no event keys off the map.
fn feed_unit_reach(
    script: Option<NonSendMut<UiScript>>,
    tokens: UnitTokens,
    self_q: Query<(&Transform, &ObjectStore), With<SelfPlayer>>,
    transforms: Query<&Transform>,
    factions: Option<Res<Factions>>,
    reputations: Res<Reputations>,
    group: Option<Res<crate::ui_party::GroupState>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let mut reach = HashMap::new();
    if let Some((self_tf, self_store)) = self_q.iter().next() {
        for guid in script.held_unit_guids() {
            let Some((entity, _)) = tokens.held(guid) else {
                continue;
            };
            let Ok(tf) = transforms.get(entity) else {
                continue;
            };
            let store = tokens.stores.get(entity).ok();
            let inspectable = benilla_protocol::guid::is_player(guid)
                && !crate::target::can_attack(
                    store,
                    factions.as_deref(),
                    &reputations,
                    crate::target::party_of(group.as_deref()),
                    Some(self_store),
                );
            reach.insert(
                guid,
                benilla_ui::script::UnitReach {
                    dist_sq: dist_sq(tf.translation, self_tf.translation),
                    inspectable,
                },
            );
        }
    }
    script.set_unit_reach(reach);
}

/// [`feed_units`]' stores and change tracking, grouped to stay under Bevy's 16-parameter limit.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct UnitStores<'w, 's> {
    all: Query<'w, 's, &'static ObjectStore>,
    /// The dirty gate: whose descriptor moved, items excluded (their writes name no unit token).
    changed: Query<'w, 's, (), (Changed<ObjectStore>, Without<crate::items::ItemObject>)>,
    /// Whose object left the manager, by [`Guid`]: a fading model keeps its store 2 s longer.
    removed: RemovedComponents<'w, 's, Guid>,
    /// This run's per-field edges, which [`fire_transitions`]' watch-bridge arms fire off.
    edges: MessageReader<'w, 's, FieldChanged>,
}

/// The client tables [`snapshot`] reads beside a descriptor, as one parameter for the feeders that
/// sit at Bevy's 16-parameter limit: `ChrClasses.dbc` for the relic slot, and the form table
/// the creature-type resolver's first stage reads. Each is absent without game data.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct SnapshotTables<'w> {
    classes: Option<Res<'w, crate::chr_classes::ChrClassTable>>,
    spells: Option<Res<'w, crate::ui_action::Spells>>,
}

impl SnapshotTables<'_> {
    /// `ChrClasses.dbc`; without it no class has a relic slot, the reference's bounds leg.
    pub(crate) fn classes(&self) -> Option<&ChrClasses> {
        self.classes.as_deref().map(|t| &t.0)
    }

    /// The creature-type resolver's sources: the name cache and the form table.
    pub(crate) fn types<'a>(&'a self, names: &'a NameCache) -> CreatureTypeSources<'a> {
        CreatureTypeSources::of_resources(names, self.spells.as_deref())
    }
}

/// `UnitReaction(unit, "player")`: [`ring_reaction`] plus one, so `1..=7`, Hated to Revered, and
/// Exalted reads 7 too. Stock `UnitReactionColor` has seven entries (`TargetFrame.lua:6-14`), one
/// per value.
pub(crate) fn unit_reaction(
    factions: Option<&Factions>,
    reputations: &Reputations,
    party: &dyn crate::target::GroupMembers,
    store: &ObjectStore,
    self_store: Option<&ObjectStore>,
) -> u8 {
    ring_reaction(factions, reputations, party, Some(store), self_store) + 1
}

/// Build a unit snapshot from a streamed descriptor, its guid, its cached name and its
/// `UnitReaction` (`1..=7`, or `0` where none is resolved, as for `"player"`); `classes` feeds the
/// relic column and `types` the creature type, which every unit carries, players included.
pub(crate) fn snapshot(
    store: &ObjectStore,
    guid: u64,
    name: Option<String>,
    reaction: u8,
    classes: Option<&ChrClasses>,
    types: CreatureTypeSources<'_>,
) -> UnitState {
    let power_type = store.0.unit_power_type();
    let race = store.0.unit_race().and_then(race_names);
    let class_id = store.0.unit_class();
    let class = class_id.and_then(class_names);
    UnitState {
        exists: true,
        // A live descriptor is having succeeded, all of `UnitIsVisible`.
        has_object: true,
        // `UnitIsConnected` answers 1 for any unit the object manager holds; only an unheld member
        // reads the roster record's online bit.
        is_connected: true,
        // What the token resolver yields, and `UnitIsUnit` compares.
        guid,
        name,
        // The UI getters: `UNIT_DYNFLAG_DEAD` (feign death) zeroes `UnitHealth` and `UnitMana` but
        // not the maxima, so its edge fires the reference's pair; the power getters divide rage by
        // 10 and happiness by 1000.
        health: store.0.unit_shown_health().unwrap_or(0),
        max_health: store.0.unit_max_health().unwrap_or(0),
        level: store.0.unit_level().unwrap_or(0),
        power_type,
        power: store.0.unit_shown_power(power_type).unwrap_or(0),
        max_power: store.0.unit_shown_max_power(power_type).unwrap_or(0),
        shown_powers: std::array::from_fn(|ty| store.0.unit_shown_power(ty as u8).unwrap_or(0)),
        shown_max_powers: std::array::from_fn(|ty| {
            store.0.unit_shown_max_power(ty as u8).unwrap_or(0)
        }),
        // `UnitIsDead`: health ≤ 0 or the dead-looking flag.
        dead: store.0.unit_reads_dead(),
        // `PLAYER_FLAGS` bit `0x10`; a ghost's health is 1, so `dead` is false for it.
        ghost: store.0.player_is_ghost(),
        // `UnitIsCharmed`: `UNIT_FIELD_CHARMEDBY != 0`.
        charmed: store.0.unit_charmed_by().is_some(),
        // `UNIT_DYNAMIC_FLAGS` bits `0x4`/`0x8`; a unit with no descriptor reads `false`.
        tapped: store.0.unit_tapped(),
        tapped_by_player: store.0.unit_tapped_by_player(),
        // `UnitIsPartyLeader`'s descriptor leg; a creature has no PLAYER block and reads absent.
        group_leader: store.0.player_is_group_leader(),
        // The raw dword `PLAYER_FLAGS_CHANGED` fires on; 0 on a creature, as in the reference.
        player_flags: store.0.player_flags(),
        // `PLAYER_FLAGS` AFK `0x2` and DND `0x4` (`0x60ccf3`, `0x60cdd1`); 0 on a creature.
        afk: store.0.player_flags() & 0x2 != 0,
        dnd: store.0.player_flags() & 0x4 != 0,
        reaction,
        // The one resolver behind `UnitCreatureType` and the tooltip's type slot: the form's type,
        // else the template's, else the race's.
        creature_type_name: creature_type_word(types.of(store)).map(str::to_string),
        race: race.map(|(n, _)| n.to_string()),
        race_file: race.map(|(_, f)| f.to_string()),
        class: class.map(|(n, _)| n.to_string()),
        class_file: class.map(|(_, f)| f.to_string()),
        // `UnitHasRelicSlot`: TYPEMASK_PLAYER first, then the class byte against `ChrClasses.dbc`
        // field 16; without the player test a class-2 NPC answers 1.
        has_relic_slot: matches!(store.0.object_type(), Some(ObjectType::Player))
            && class_id.is_some_and(|c| classes.is_some_and(|t| t.has_relic_slot(u32::from(c)))),
        // `GetDamageBonusStat`: the class byte against `ChrClasses.dbc` field 2, read for the
        // active player only (TYPEMASK_PLAYER).
        damage_bonus_stat: matches!(store.0.object_type(), Some(ObjectType::Player))
            .then(|| class_id.and_then(|c| classes.and_then(|t| t.damage_bonus_stat(u32::from(c)))))
            .flatten(),
        // Gender byte 0 male, 1 female, on `UnitSex`'s scale: 2 male, 3 female, 0 unknown (nil).
        sex: match store.0.unit_gender() {
            Some(0) => 2,
            Some(1) => 3,
            _ => 0,
        },
        // `UNIT_FIELD_FLAGS` PvP `0x1000` and Skinnable `0x04000000` (vmangos `UnitDefines.h`).
        pvp: store.0.unit_flags() & 0x1000 != 0,
        // The tooltip's unit setter adds the line for a non-player only (`0x621dd8`); the creature
        // record's kind is [`enrich_unit`]'s.
        skinnable: (store.0.unit_flags() & 0x0400_0000 != 0
            && !matches!(store.0.object_type(), Some(ObjectType::Player)))
        .then_some(crate::ui_action::skinnable_text_key(0)),
        // `UnitPlayerControlled`: bit `0x8`, set by pets and charmed creatures as well as players.
        player_controlled: store.0.unit_flags() & 0x8 != 0,
        flags: store.0.unit_flags(),
        // The whole dword: the reference's watch is a memcmp over it, not a bit test.
        dynamic_flags: store.0.unit_dynamic_flags(),
        owner: store
            .0
            .unit_summoned_by()
            .or_else(|| store.0.unit_charmed_by())
            .or_else(|| store.0.unit_created_by())
            .unwrap_or(0),
        // `UnitAffectingCombat`: bit 19, the one combat bit for every token.
        in_combat: store.0.unit_flags() & crate::player::UNIT_FLAG_IN_COMBAT != 0,
        // Free-for-all PvP, `PLAYER_FLAGS` bit 7 (vmangos `Player.h:322`); false on a creature.
        is_pvp_ffa: store.0.player_flags() & 0x80 != 0,
        // The honor rank, `PLAYER_BYTES_3` byte 3 (0..=18), public for every player in view.
        pvp_rank: store.0.player_pvp_rank().unwrap_or(0),
        // The team digit of `PVP_RANK_<rank>_<team>`, from the race ([`race_pvp_team`]).
        pvp_team: store.0.unit_race().map_or(-1, race_pvp_team),
        // `PLAYER_BYTES_3` byte 2, the city-protector title (`PVP_MEDAL<n>`), unset by vmangos.
        pvp_medal: store.0.player_pvp_medal().unwrap_or(0),
        // `PLAYER_CHOSEN_TITLE`, public, the title `UnitPVPName` shows (`0x72a290`).
        chosen_title: store.0.player_chosen_title(),
        // `is_player` and the creature-record fields come from [`enrich_unit`].
        ..Default::default()
    }
}

/// Fill the guid-keyed tooltip fields: `is_player`, or a creature's record fields and its
/// faction-name line.
pub(crate) fn enrich_unit(
    state: &mut UnitState,
    guid: u64,
    names: &NameCache,
    store: &ObjectStore,
    factions: Option<&Factions>,
    self_store: Option<&ObjectStore>,
) {
    if benilla_protocol::guid::is_player(guid) {
        state.is_player = true;
        // No faction line for players: their factions have no reputation slot.
        return;
    }
    state.counter_hidden = crate::combat_log::entries::shown_guid(guid, names) != guid;
    let Some(entry) = benilla_protocol::guid::entry(guid) else {
        return;
    };
    // The creature record (`unit+0xb30`); `None` until the query answers, a real state the plate
    // draws under the name `UNKNOWNOBJECT`.
    let rec = names.creature_record(entry);
    if let Some(rec) = rec {
        state.subtitle = rec.subname.clone();
        // The client's one rank getter, never `rec.rank`: an enslaved elite reads rank 0.
        state.rank = crate::names::gated_rank(Some(rec), Some(store));
        state.civilian = rec.civilian;
        state.racial_leader = rec.racial_leader;
        if state.skinnable.is_some() {
            state.skinnable = Some(crate::ui_action::skinnable_text_key(rec.type_flags));
        }
    }
    // The faction-name line, every gate of the tooltip builder: the record's `HIDE_FACTION_TOOLTIP`
    // (`0x10`), a reputation slot, and the race/class slot walk with its hidden flag (`0x4`). Its
    // entry gate passes with no record, so the line shows before the query answers.
    if rec.is_none_or(|r| r.type_flags & crate::names::type_flags::NO_FACTION_TOOLTIP == 0) {
        state.faction_name = (|| {
            let catalog = factions?.catalog();
            let faction_id = catalog.template(store.0.unit_faction_template()?)?.faction;
            let info = catalog.reputation_faction(faction_id)?;
            let self_store = self_store?;
            let race = self_store.0.unit_race().unwrap_or(0);
            let class = self_store.0.unit_class().unwrap_or(0);
            info.tooltip_shows_for(race, class)
                .then(|| catalog.faction_name(faction_id).map(str::to_string))
                .flatten()
        })();
    }
}

/// Drain `TogglePVP` into the bare `CMSG_TOGGLE_PVP` and `SetPVP(state)` into its one-byte form,
/// in that order within a frame.
fn drain_pvp_toggles(script: Option<NonSendMut<UiScript>>, commands: Res<NetCommands>) {
    let Some(mut script) = script else {
        return;
    };
    for _ in 0..script.take_pvp_toggles() {
        let _ = commands.0.send(crate::net::ClientCommand::TogglePvp);
    }
    for state in script.take_pvp_sets() {
        let _ = commands.0.send(crate::net::ClientCommand::SetPvp { state });
    }
}

/// Drain the `ShowHelm`/`ShowCloak` flips into `CMSG_TOGGLE_HELM`/`CMSG_TOGGLE_CLOAK`; the VM,
/// which alone knows what the Options row did, has already decided each is needed.
fn drain_worn_display_toggles(script: Option<NonSendMut<UiScript>>, commands: Res<NetCommands>) {
    let Some(mut script) = script else {
        return;
    };
    for which in script.take_worn_display_toggles() {
        let _ = commands.0.send(match which {
            WornDisplay::Helm => crate::net::ClientCommand::ToggleHelm,
            WornDisplay::Cloak => crate::net::ClientCommand::ToggleCloak,
        });
    }
}

/// Drain `SetActionBarToggles` into `CMSG_SET_ACTIONBAR_TOGGLES`, one packet per call: the
/// reference binding neither checks for a change nor coalesces.
fn drain_action_bar_toggles(script: Option<NonSendMut<UiScript>>, commands: Res<NetCommands>) {
    let Some(mut script) = script else {
        return;
    };
    for toggles in script.take_action_bar_toggle_sends() {
        let _ = commands
            .0
            .send(crate::net::ClientCommand::SetActionBarToggles { toggles });
    }
}

/// The PvP preference bit `CMSG_TOGGLE_PVP` flips (vmangos `Player.h:324`); not the
/// `UNIT_FIELD_FLAGS` PvP bit `0x1000` the icon draws, which lingers for the server's timer.
const PLAYER_FLAGS_PVP_DESIRED: u32 = 0x200;

/// Inside a rest area (vmangos `Player.h:320`), the bit `IsResting` (`0x60dd40`) tests.
const PLAYER_FLAGS_RESTING: u32 = 0x20;

/// `PLAYER_FLAGS` bits 12 and 13, a realm's two play-time limits, read by `PartialPlayTime`
/// (`0x51cb30`) and `NoPlayTime` (`0x51cba0`); not the pre-1.6.1 `CAN_SELF_RESURRECT`.
const PLAYER_FLAGS_PARTIAL_PLAY_TIME: u32 = 0x1000;
const PLAYER_FLAGS_NO_PLAY_TIME: u32 = 0x2000;

/// The server's no-XP flag, bit 25, which `IsXPUserDisabled` (`0x614140`) reads (`0x614170`).
const PLAYER_FLAGS_NO_XP_GAIN: u32 = 0x0200_0000;

/// The `(toast, verbose)` GlobalStrings keys for a real change of the PvP-preference bit, silent on
/// first sight (the reference diffs bits). The toasts are catalog rows 437/438 (kind 1, yellow
/// `UI_INFO_MESSAGE`); the verbose keys are no rows, so the handler picks their chat surface.
fn pvp_announcement(was: Option<bool>, now: bool) -> Option<(&'static str, &'static str)> {
    if was? == now {
        return None;
    }
    Some(if now {
        ("ERR_PVP_TOGGLE_ON", "PVP_TOGGLE_ON_VERBOSE")
    } else {
        ("ERR_PVP_TOGGLE_OFF", "PVP_TOGGLE_OFF_VERBOSE")
    })
}

/// The rest-state chat key: the reference messages only on a real byte change (the `rep cmpsb`
/// diff), through the pair table: 1 rested, 2 normal, 0 the no-message sentinel (`0x1d1`), ≥ 3
/// gated off (`cmp esi,3; jae`). Rows 346/347 are kind 0, a system chat line with no cue or voice
/// (`type_tag 0x44`).
fn rest_state_message(prev: u8, new: u8) -> Option<&'static str> {
    if prev == new {
        return None;
    }
    match new {
        1 => Some("ERR_EXHAUSTION_RESTED"),
        2 => Some("ERR_EXHAUSTION_NORMAL"),
        _ => None,
    }
}

/// `UnitFactionGroup`'s first return: the faction template's group mask `& 6`, named by
/// `FactionGroup.dbc`. Only the side bits: player and city-guard templates carry `Player|<side>`,
/// and the Player and Monster rows have no name and no icon art.
pub(crate) fn faction_group(store: &ObjectStore, factions: Option<&Factions>) -> Option<String> {
    let catalog = factions?.catalog();
    let template = catalog.template(store.0.unit_faction_template()?)?;
    // The English `InternalName`: every stock consumer concatenates it into a texture path.
    catalog
        .faction_group_internal_name(template.group_mask & 6)
        .map(str::to_string)
}

/// The character's race's `ChrRaces.dbc` template group mask `& 6`, `UnitFactionGroup("player")`'s
/// side (`0x60d0f4`), with the catalog that names it.
fn race_group_mask<'a>(
    store: &ObjectStore,
    factions: Option<&'a Factions>,
) -> Option<(&'a benilla_formats::FactionCatalog, u32)> {
    let catalog = factions?.catalog();
    let template = catalog.race_template(store.0.unit_race()?)?;
    Some((catalog, template.group_mask & 6))
}

/// `UnitFactionGroup`'s second return, the localized name stock shows as text.
pub(crate) fn faction_group_localized(
    store: &ObjectStore,
    factions: Option<&Factions>,
) -> Option<String> {
    let catalog = factions?.catalog();
    let template = catalog.template(store.0.unit_faction_template()?)?;
    catalog
        .faction_group_name(template.group_mask & 6)
        .map(str::to_string)
}

/// `CreatureType.dbc` id to the enUS word, a creature's level-line class slot.
fn creature_type_word(t: u32) -> Option<&'static str> {
    Some(match t {
        1 => "Beast",
        2 => "Dragonkin",
        3 => "Demon",
        4 => "Elemental",
        5 => "Giant",
        6 => "Undead",
        7 => "Humanoid",
        8 => "Critter",
        9 => "Mechanical",
        // The shipped table runs 1..11; the nameplate filter tests 11.
        11 => "Totem",
        // Deviation: 10, "Not specified", answers None because the word would print in the
        // tooltip's level line; `UnitCreatureType` then answers nil where the reference names it.
        _ => return None,
    })
}

/// Diff a token's snapshot against the last pushed and fire the per-field `UNIT_*` events. The
/// watch-bridge arms fire off `edges`, as the reference's create runs no notify pass; the rest fire
/// on a token's first snapshot (`prev = None`) too, where the reference's behaviour is untraced.
pub(crate) fn fire_transitions(
    script: &mut UiScript,
    token: &str,
    prev: Option<&UnitState>,
    cur: &UnitState,
    edges: &FieldEdges,
) {
    let tok = || ScriptValue::Str(token.to_string());
    let changed = |f: fn(&UnitState) -> u64| prev.is_none_or(|p| f(p) != f(cur));

    if changed(|u| u64::from(u.health)) {
        script.fire_event("UNIT_HEALTH", vec![tok()]);
    }
    if changed(|u| u64::from(u.max_health)) {
        script.fire_event("UNIT_MAXHEALTH", vec![tok()]);
    }
    if changed(|u| u64::from(u.level)) {
        script.fire_event("UNIT_LEVEL", vec![tok()]);
    }
    if changed(|u| u64::from(u.power_type)) {
        script.fire_event("UNIT_DISPLAYPOWER", vec![tok()]);
    }
    // `UNIT_FLAGS` (id 40), the per-field watch bridge: the reference registers one watch per named
    // unit field, and on any change of the dword the notifier fires, once per token naming the
    // unit, `arg1` the token. The stock pet bar reads it.
    if edges.moved(cur.guid, benilla_protocol::field::FIELD_UNIT_FLAGS) {
        script.fire_event("UNIT_FLAGS", vec![tok()]);
    }
    // `PLAYER_FLAGS_CHANGED` (id 407): no bit test after the XOR diff and above the local-GUID
    // gate, so any bit of any player fires it, once per token naming it (none). The sole 1.12
    // consumer is the target frame's leader icon (`TargetFrame.lua:88-95`); 1.12 has no AFK/DND
    // unit-frame badge.
    if edges.moved(cur.guid, benilla_protocol::field::FIELD_PLAYER_FLAGS) {
        script.fire_event("PLAYER_FLAGS_CHANGED", vec![tok()]);
    }
    // `UNIT_DYNAMIC_FLAGS` (id 137), the bridge's third arm: the watch is one dword, so any bit
    // fires it. No stock file registers it; addons do, to repaint the tapped state.
    if edges.moved(cur.guid, benilla_protocol::field::FIELD_UNIT_DYNAMIC_FLAGS) {
        script.fire_event("UNIT_DYNAMIC_FLAGS", vec![tok()]);
    }
    // The power events are named per resource (`UNIT_MANA`, `UNIT_MAXRAGE`, `UNIT_RUNIC_POWER`,
    // …), and `power_token` yields the suffix; runes have none.
    if let Some(suffix) = power_token(cur.power_type) {
        if changed(|u| u64::from(u.power)) {
            script.fire_event(&format!("UNIT_{suffix}"), vec![tok()]);
        }
        if changed(|u| u64::from(u.max_power)) {
            script.fire_event(&format!("UNIT_MAX{suffix}"), vec![tok()]);
        }
    }
    // The chosen title and the medal byte have their own name watches (`0x6cdd80`, into the UI's
    // unit-name update).
    if prev.is_none_or(|p| {
        (&p.name, p.chosen_title, p.pvp_medal) != (&cur.name, cur.chosen_title, cur.pvp_medal)
    }) {
        script.fire_event("UNIT_NAME_UPDATE", vec![tok()]);
    }
    // `UNIT_CLASSIFICATION_CHANGED`, the target frame's border repaint, on a change of the gated
    // rank, which is the classification: the creature query landing, a mob enslaved or released.
    if prev.is_none_or(|p| p.rank != cur.rank) {
        script.fire_event("UNIT_CLASSIFICATION_CHANGED", vec![tok()]);
    }
    // `UNIT_FACTION`, the PvP-icon repaint, on the fields the icon reads and on the tapped bit: the
    // reference's dynamic-flags watcher fires event 29 on bit `0x4` and has no arm for `0x8`.
    if prev.is_none_or(|p| {
        (p.pvp, p.is_pvp_ffa, &p.faction_group, p.tapped)
            != (cur.pvp, cur.is_pvp_ffa, &cur.faction_group, cur.tapped)
    }) {
        script.fire_event("UNIT_FACTION", vec![tok()]);
    }
}

/// The two interaction sources `feed_units` snapshots: the `"npc"` and the `"questnpc"` units.
#[derive(bevy::ecs::system::SystemParam)]
struct Interactions<'w> {
    npc: Option<Res<'w, crate::ui_session::InteractNpc>>,
    quest: Option<Res<'w, crate::ui_quest::QuestGiver>>,
}

fn feed_units(
    script: Option<NonSendMut<UiScript>>,
    tables: SnapshotTables,

    self_q: Query<(&ObjectStore, &Guid), With<SelfPlayer>>,
    selection: Res<Selection>,
    // `Option`, as `factions`, `interact` and `entered_world`: a UI-only harness lacks their
    // plugins, and a bare `Res` is a system-validation panic.
    index: Option<Res<crate::net::GuidIndex>>,
    mut stores: UnitStores,
    mut feed: ResMut<UnitFeedState>,
    names: Res<NameCache>,
    commands: Res<NetCommands>,
    factions: Option<Res<Factions>>,
    reputations: Res<Reputations>,
    group: Res<crate::ui_party::GroupState>,
    // Chat and the message-sound queue, for the rest and PvP lines.
    mut sink: crate::ui_action::MessageSink,
    // `ResMut` because the guild-identity cache is lazy: a miss sends `CMSG_GUILD_QUERY`.
    mut guild: ResMut<crate::ui_guild::GuildState>,
    interact: Interactions,
    // Rested billing minutes, sent only in `SMSG_AUTH_RESPONSE`; the reference keeps a global.
    entered_world: Option<MessageReader<crate::net::EnteredWorldMessage>>,
) {
    let Some(mut script) = script else {
        return;
    };
    // Ahead of the gate: a login edge, with no snapshot to diff.
    if let Some(entered) =
        entered_world.and_then(|mut r| r.read().last().map(|m| m.billing_time_rested))
    {
        script.set_billing_time_rested(entered);
    }
    let chr = tables.classes();
    let types = tables.types(&names);
    let (memo, vm_reset) = feed.vm.get_reset(&script);
    let edges = FieldEdges::collect(&mut stores.edges);

    // The gate: every input read below, despawns included (invisible to `Changed`).
    let names_moved = memo.names_generation.moved(names.generation());
    let guild_moved = memo.guild_generation.moved(guild.identity_generation());
    let focus_guid = script.focus_guid();
    let focus_moved = memo.focus_guid.moved(focus_guid);
    let selection_changed = selection.is_changed();
    let stores_changed = !stores.changed.is_empty();
    let stores_removed = !stores.removed.is_empty();
    let group_changed = group.is_changed();
    let reps_changed = reputations.is_changed();
    let factions_changed = factions.as_ref().is_some_and(|r| r.is_changed());
    // The interaction NPC moves when nothing else does, and `MERCHANT_SHOW` reads `"npc"`.
    let interact_changed = interact.npc.as_ref().is_some_and(|r| r.is_changed())
        || interact.quest.as_ref().is_some_and(|r| r.is_changed());
    gate::trace(
        "feed_units",
        &[
            ("vm_reset", vm_reset),
            ("names", names_moved),
            ("guild", guild_moved),
            ("focus", focus_moved),
            ("selection", selection_changed),
            ("stores", stores_changed),
            ("removed", stores_removed),
            ("group", group_changed),
            ("reputations", reps_changed),
            ("factions", factions_changed),
            ("interact", interact_changed),
        ],
    );
    let gate = gate::Gate::new(
        vm_reset
            || names_moved
            || guild_moved
            || focus_moved
            || selection_changed
            || stores_changed
            || stores_removed
            || group_changed
            || reps_changed
            || factions_changed
            || interact_changed,
    );
    stores.removed.clear();
    if gate.skip() {
        return;
    }

    // A missing unit is `None`, which `set_unit` clears; a name miss lands on a later frame.
    let self_pair = self_q.iter().next();
    let player = self_pair.map(|(store, guid)| {
        let name = names
            .resolve_unit(guid.0, Some(store), &commands)
            .map(str::to_string);
        let mut s = snapshot(store, guid.0, name, 0, chr, types);
        s.is_player = true;
        // The assist check answers the player for himself.
        s.can_assist = true;
        s.selection_color = crate::target::selection_color(
            store,
            factions.as_deref(),
            &reputations,
            &*group,
            Some(store),
        );
        s.raid_target = group.raid_target_index(guid.0);
        s.group_roles = group.own_roles;
        // `UnitFactionGroup("player")` (`0x60d0f4`) reads the character's race, not the live
        // template, so GM mode's template 35 keeps the side.
        let side = race_group_mask(store, factions.as_deref());
        s.faction_group = side.and_then(|(c, m)| c.faction_group_internal_name(m).map(Into::into));
        s.faction_group_localized = side.and_then(|(c, m)| c.faction_group_name(m).map(Into::into));
        // `GetGuildInfo`: the public guild fields (191/192) joined against the lazy guild cache.
        s.guild = crate::ui_guild::unit_guild(&store.0, &mut guild, &commands);
        s
    });
    // What the three snapshots below (target, target-of-target, NPC) read besides a descriptor.
    let held = held::HeldUnits {
        names: &names,
        commands: &commands,
        factions: factions.as_deref(),
        reputations: &reputations,
        group: &group,
        self_store: self_pair.map(|(s, _)| s),
        classes: chr,
        types,
    };
    let target = selection.target.zip(selection.guid).and_then(|(e, guid)| {
        let store = stores.all.get(e).ok()?;
        let mut s = held.state(store, guid);
        s.guild = crate::ui_guild::unit_guild(&store.0, &mut guild, &commands);
        Some(s)
    });

    // `"focus"`: the VM's focus guid while the object manager holds it, as `"target"` is read.
    let focus = index
        .as_ref()
        .and_then(|i| i.0.get(&focus_guid))
        .and_then(|&e| stores.all.get(e).ok())
        .map(|store| {
            let mut s = held.state(store, focus_guid);
            s.guild = crate::ui_guild::unit_guild(&store.0, &mut guild, &commands);
            s
        });

    // `"targettarget"`: one hop off the target's `UNIT_FIELD_TARGET`, streamed guids only. No guild
    // leg: nothing asks it for one, and a lazy-cache miss sends a query.
    let tot = selection
        .target
        .and_then(|e| stores.all.get(e).ok())
        .and_then(|s| s.0.unit_target())
        .filter(|guid| *guid != 0)
        .and_then(|guid| Some((*index.as_ref()?.0.get(&guid)?, guid)))
        .and_then(|(entity, guid)| Some(held.state(stores.all.get(entity).ok()?, guid)));

    // `"player"` is pushed only while its descriptor exists: the roster seat stands in before
    // arrival, and `PLAYER_LOGOUT` handlers still read `UnitName("player")`, as the reference's do.
    // `"target"` pushes its absence too; both diff against the event loop's memo.
    if let Some(cur) = &player {
        if memo.last.get("player") != Some(cur) {
            gate.audit("feed_units", "the player snapshot");
            script.set_unit("player", player.clone());
        }
    }
    let target_dirty = match (&target, memo.last.get("target")) {
        (Some(cur), Some(prev)) => cur != prev,
        (None, None) => false,
        _ => true,
    };
    if target_dirty {
        gate.audit("feed_units", "the target snapshot");
        script.set_unit("target", target.clone());
    }
    // Absence is data here too: the target dropping its target clears the token.
    let tot_dirty = match (&tot, memo.last.get("targettarget")) {
        (Some(cur), Some(prev)) => cur != prev,
        (None, None) => false,
        _ => true,
    };
    if tot_dirty {
        gate.audit("feed_units", "the target-of-target snapshot");
        script.set_unit("targettarget", tot.clone());
    }
    let focus_dirty = match (&focus, memo.last.get("focus")) {
        (Some(cur), Some(prev)) => cur != prev,
        (None, None) => false,
        _ => true,
    };
    if focus_dirty {
        gate.audit("feed_units", "the focus snapshot");
        script.set_unit("focus", focus.clone());
    }

    // `"npc"`: the interaction NPC, the reference's global that the interact-NPC setter writes,
    // which `InteractNpc` models. No guild leg, as for `"targettarget"`.
    let npc = interact
        .npc
        .as_deref()
        .and_then(|i| Some((i.0?, i.1?)))
        .and_then(|(entity, guid)| Some(held.state(stores.all.get(entity).ok()?, guid)));
    // Closing the window must clear the token, so the memo is written here, not only read. No
    // `fire_transitions`: nothing draws `"npc"` as a unit frame.
    let npc_dirty = match (&npc, memo.last.get("npc")) {
        (Some(cur), Some(prev)) => cur != prev,
        (None, None) => false,
        _ => true,
    };
    if npc_dirty {
        gate.audit("feed_units", "the interaction-NPC snapshot");
        script.set_unit("npc", npc.clone());
        match &npc {
            Some(cur) => {
                memo.last.insert("npc".to_string(), cur.clone());
            }
            None => {
                memo.last.remove("npc");
            }
        }
    }

    // `"questnpc"`: the questgiver window's source (`0x6081b0`), which `QuestFrame_SetPortrait`
    // titles the window with. A GameObject or item source is held like a unit.
    let quest_npc = interact
        .quest
        .as_deref()
        .and_then(crate::ui_quest::QuestGiver::quest_npc)
        .and_then(|guid| {
            let entity = *index.as_deref()?.0.get(&guid)?;
            Some(held.state(stores.all.get(entity).ok()?, guid))
        });
    let quest_npc_dirty = match (&quest_npc, memo.last.get("questnpc")) {
        (Some(cur), Some(prev)) => cur != prev,
        (None, None) => false,
        _ => true,
    };
    if quest_npc_dirty {
        gate.audit("feed_units", "the questgiver snapshot");
        script.set_unit("questnpc", quest_npc.clone());
        match &quest_npc {
            Some(cur) => {
                memo.last.insert("questnpc".to_string(), cur.clone());
            }
            None => {
                memo.last.remove("questnpc");
            }
        }
    }

    // The XP pair and `PLAYER_XP_UPDATE`, ahead of the `PLAYER_ENTERING_WORLD` fire so its
    // handlers read real values. The event fires on first sight too, at login and after a
    // `/reload`, where the reference's field watchers stay silent.
    if let Some((store, _)) = self_q.iter().next() {
        let xp = store.0.player_xp().unwrap_or(0);
        let next = store.0.player_next_level_xp().unwrap_or(0);
        if memo.last_xp != Some((xp, next)) {
            gate.audit("feed_units", "the XP pair");
            memo.last_xp = Some((xp, next));
            script.set_player_xp(xp, next);
            script.fire_event("PLAYER_XP_UPDATE", vec![]);
        }
    }

    // The rest snapshot (rest-state byte, pool, `PLAYER_FLAGS`), ahead of the entering-world fire
    // as the reference's descriptor is. Below `0x6e0fd0`'s local-GUID gate (`0x6e10dc`) each arm
    // tests its own bits: `PLAYER_UPDATE_RESTING` on either edge of `0x20` (fire `0x6e112d`),
    // `PLAYTIME_CHANGED` on `0x3000` (`0x6e11c9`), `DISABLE_`/`ENABLE_XP_GAIN` on `0x2000000`
    // (`0x6e121c`). `UPDATE_EXHAUSTION` is two other watchers, one on the byte and one on the pool;
    // here it also fires on first sight, where they stay silent on a create and on `/reload`.
    if let Some((store, _)) = self_q.iter().next() {
        let rest = (
            store.0.player_rest_state().unwrap_or(0),
            store.0.player_rest_state_experience().unwrap_or(0),
            store.0.player_flags(),
        );
        if memo.last_rest != Some(rest) {
            gate.audit("feed_units", "the rest snapshot");
            let prev = memo.last_rest;
            memo.last_rest = Some(rest);
            script.set_rest_state(rest.0, rest.1, rest.2 & PLAYER_FLAGS_RESTING != 0);
            // The play-time bits share the dword and the memo; only the events split by bit.
            script.set_play_time(
                rest.2 & PLAYER_FLAGS_PARTIAL_PLAY_TIME != 0,
                rest.2 & PLAYER_FLAGS_NO_PLAY_TIME != 0,
            );
            script.set_xp_user_disabled(rest.2 & PLAYER_FLAGS_NO_XP_GAIN != 0);
            if prev.map(|p| (p.0, p.1)) != Some((rest.0, rest.1)) {
                script.fire_event("UPDATE_EXHAUSTION", vec![]);
            }
            // Only the byte watcher messages, and only on a real change: `prev` None is the login
            // descriptor, a create the reference never runs through the notify pass.
            if let Some(p) = prev {
                let line = rest_state_message(p.0, rest.0)
                    .and_then(|key| crate::ui_action::keyed_line(&script, key));
                crate::ui_action::show_messages(&mut script, &mut sink, "ui_unit", line);
            }
            // The self-only flag events on their own bits' edges; the login create is silent.
            if let Some(p) = prev {
                let moved = p.2 ^ rest.2;
                if moved & PLAYER_FLAGS_RESTING != 0 {
                    script.fire_event("PLAYER_UPDATE_RESTING", vec![]);
                }
                // `PLAYTIME_CHANGED` (id 467), argless; stock `PlayerFrame.lua` registers it.
                if moved & (PLAYER_FLAGS_PARTIAL_PLAY_TIME | PLAYER_FLAGS_NO_PLAY_TIME) != 0 {
                    script.fire_event("PLAYTIME_CHANGED", vec![]);
                }
                // Argless; `ReputationWatchBar` re-lays the XP bar on either.
                if moved & PLAYER_FLAGS_NO_XP_GAIN != 0 {
                    let event = if rest.2 & PLAYER_FLAGS_NO_XP_GAIN != 0 {
                        "DISABLE_XP_GAIN"
                    } else {
                        "ENABLE_XP_GAIN"
                    };
                    script.fire_event(event, vec![]);
                }
            }
        }
    }

    // `PLAYER_LEVEL_UP` on any level change off the descriptor, demotions included (vmangos
    // `GiveLevel` sends `SMSG_LEVELUP_INFO` for those too); the reference's trigger is untraced.
    if let Some((store, _)) = self_q.iter().next() {
        if let Some(level) = store.0.unit_level() {
            let prev = memo.last_level.replace(level);
            if prev.is_some_and(|p| level != p) {
                gate.audit("feed_units", "the level edge");
                // All nine, as the reference formats them (`%d` nine times) and
                // `ChatFrame.lua:1283-1320` reads them; missing gains are zeros, since stock's
                // `if ( argN > 0 )` raises on nil.
                let (info, talent_points) = sink.chat.take_level_up_gains(level).unzip();
                let gain = |f: fn(&benilla_protocol::messages::LevelUpInfo) -> i32| {
                    ScriptValue::Int(i64::from(info.as_ref().map_or(0, f)))
                };
                script.fire_event(
                    "PLAYER_LEVEL_UP",
                    vec![
                        ScriptValue::Int(i64::from(level)),
                        gain(|l| l.health),
                        gain(|l| l.powers[0]),
                        ScriptValue::Int(i64::from(talent_points.unwrap_or(0))),
                        gain(|l| l.stats[0]),
                        gain(|l| l.stats[1]),
                        gain(|l| l.stats[2]),
                        gain(|l| l.stats[3]),
                        gain(|l| l.stats[4]),
                    ],
                );
                // The player's level watch (`0x6d66e0`): the battleground types' `canEnter` moved.
                script.fire_event("PVPQUEUE_ANYWHERE_UPDATE_AVAILABLE", vec![]);
            }
        }
    }

    // `PLAYER_FIELD_BYTES` byte 2, the four extra bars, on the edge: the reference never writes the
    // cell or watches it, and stock reads it once, at `PLAYER_ENTERING_WORLD` (`UIParent.lua:364`),
    // so this goes ahead of that fire. No player and a zero byte share the four-nil branch.
    if let Some((store, _)) = self_q.iter().next() {
        let toggles = store.0.player_action_bar_toggles().unwrap_or(0);
        if memo.action_bar_toggles != Some(toggles) {
            gate.audit("feed_units", "the action-bar toggle byte");
            memo.action_bar_toggles = Some(toggles);
            script.set_action_bar_toggles(toggles);
        }
    }

    // `PLAYER_ENTERING_WORLD` once per world entry, after our descriptor lands, as the reference's
    // does. At world exit, re-arm it and forget the player-global memos so the next character seeds
    // them.
    if self_pair.is_some() {
        if !memo.entered_world {
            gate.audit("feed_units", "the PLAYER_ENTERING_WORLD arm");
            script.fire_event("PLAYER_ENTERING_WORLD", vec![]);
            // The cascade's reputation world entry (`0x5d2bc0`) re-sorts and fires `UPDATE_FACTION`
            // (`0x5d1bfa`) with the player seated, which re-shows the XP bar the pre-player
            // `SMSG_INITIALIZE_FACTIONS` fire hid at a 0 maximum.
            script.fire_event("UPDATE_FACTION", vec![]);
            memo.entered_world = true;
        }
    } else if memo.entered_world {
        gate.audit("feed_units", "the world-exit disarm");
        memo.entered_world = false;
        memo.last_xp = None;
        memo.last_rest = None;
        memo.last_level = None;
        memo.in_combat = None;
        memo.pvp_desired = None;
        // Edge-only pushes: a stale memo would skip a new character whose values match it.
        memo.worn_hidden = None;
        memo.action_bar_toggles = None;
    }

    for (token, snap) in [
        ("player", &player),
        ("target", &target),
        ("targettarget", &tot),
        ("focus", &focus),
    ] {
        match snap {
            Some(cur) => {
                let prev = memo.last.get(token);
                if prev != Some(cur) {
                    gate.audit("feed_units", "a unit-token transition");
                    fire_transitions(&mut script, token, prev, cur, &edges);
                    memo.last.insert(token.to_string(), cur.clone());
                }
            }
            None => {
                // A clear fires nothing; the target frame hears `PLAYER_TARGET_CHANGED` below.
                if memo.last.remove(token).is_some() {
                    gate.audit("feed_units", "a unit-token clear");
                }
            }
        }
    }

    // `PLAYER_TARGET_CHANGED`, argless, when the selection changes. A `/reload`'s fresh memo fires
    // it for a kept selection, where the reference's shutdown clears the target.
    if selection.guid != memo.target_guid {
        gate.audit("feed_units", "the PLAYER_TARGET_CHANGED edge");
        memo.target_guid = selection.guid;
        script.fire_event("PLAYER_TARGET_CHANGED", vec![]);
    }

    // `PLAYER_REGEN_DISABLED`/`ENABLED` on our in-combat flag's edge (`UNIT_FLAG_IN_COMBAT`
    // `0x00080000`, vmangos `UnitDefines.h:564`); the reference's own trigger is untraced.
    if let Some((store, _)) = self_pair {
        let in_combat = store.0.unit_flags() & 0x0008_0000 != 0;
        if memo.in_combat != Some(in_combat) {
            gate.audit("feed_units", "the combat-flag edge");
            let first_sight = memo.in_combat.is_none();
            memo.in_combat = Some(in_combat);
            if !first_sight || in_combat {
                script.fire_event(
                    if in_combat {
                        "PLAYER_REGEN_DISABLED"
                    } else {
                        "PLAYER_REGEN_ENABLED"
                    },
                    vec![],
                );
            }
        }
    }

    // The reference's local `PLAYER_FLAGS` handler answers a preference change, not the icon's
    // flag, with a yellow toast and a system chat line.
    if let Some((store, _)) = self_pair {
        let desired = store.0.player_flags() & PLAYER_FLAGS_PVP_DESIRED != 0;
        if let Some((toast, verbose)) = pvp_announcement(memo.pvp_desired, desired) {
            gate.audit("feed_units", "the PvP-desired edge");
            // The verbose sentence is no catalog row, so it goes `unkeyed` to chat:
            // `Shown::keyed`'s unknown-key fallback would show it red.
            let lines = [
                crate::ui_action::keyed_line(&script, toast),
                script
                    .lua()
                    .globals()
                    .get::<String>(verbose)
                    .ok()
                    .filter(|t| !t.is_empty())
                    .map(|t| {
                        crate::ui_action::Shown::unkeyed(benilla_ui::messages::MsgKind::Chat, t)
                    }),
            ];
            crate::ui_action::show_messages(
                &mut script,
                &mut sink,
                "ui_unit",
                lines.into_iter().flatten(),
            );
        }
        memo.pvp_desired = Some(desired);
    }

    // The hide bits for `ShowingHelm()`/`ShowingCloak()`, on the edge only: the setter flips the
    // VM's belief on the click, and a stale push before the server answers would undo it.
    if let Some((store, _)) = self_pair {
        let hidden = (store.0.player_hides_helm(), store.0.player_hides_cloak());
        if memo.worn_hidden != Some(hidden) {
            gate.audit("feed_units", "the worn-display pair");
            memo.worn_hidden = Some(hidden);
            script.set_worn_display(!hidden.0, !hidden.1);
        }
    }
}

/// The combo banks into the VM, then `UNIT_COMBO_POINTS` once per packet, changed or not, to every
/// token naming the bank's unit: ours and the pet's (`0x5d34b0`), each through the unit-signal send
/// (`0x60bf10`); the pet's packet then fires `PET_BAR_UPDATE_USABLE` (`0x5d3507`). A fresh VM takes
/// the banks silently.
fn feed_combo_points(
    script: Option<NonSendMut<UiScript>>,
    combo: Option<Res<crate::spell::ComboPoints>>,
    self_guid: Option<Res<crate::net::SelfGuid>>,
    pet_bar: Option<Res<crate::ui_pet::PetBar>>,
    mut memo: Local<crate::ui_script::VmMemo<Option<(u64, u64)>>>,
) {
    let (Some(mut script), Some(combo)) = (script, combo) else {
        return;
    };
    let memo = memo.get(&script);
    let epochs = (combo.player_epoch, combo.pet_epoch);
    if *memo == Some(epochs) {
        return;
    }
    let fire = memo.map(|(player, pet)| (player != epochs.0, pet != epochs.1));
    *memo = Some(epochs);
    script.set_combo_points(combo.player.points, combo.player.target);
    script.set_pet_combo_points(combo.pet.points, combo.pet.target);
    let Some((player_moved, pet_moved)) = fire else {
        return;
    };
    if player_moved {
        if let Some(me) = self_guid.and_then(|g| g.0) {
            script.fire_unit_event(me, "UNIT_COMBO_POINTS", vec![]);
        }
    }
    if pet_moved {
        if let Some(pet) = pet_bar.as_deref().map(|b| b.spells.pet_guid) {
            script.fire_unit_event(pet, "UNIT_COMBO_POINTS", vec![]);
        }
        script.fire_event("PET_BAR_UPDATE_USABLE", vec![]);
    }
}

/// `SMSG_POWER_UPDATE` (`0x7236c0`): the unit's predicted power, which `UnitPower` reads until the
/// field moves ([`benilla_protocol::ObjectFields::set_predicted_power`]). The reference signals
/// `UNIT_<POWER>` from here only at the maximum (`0x722ca9`); the feed's diff fires on any move.
fn on_power_update(
    In(ev): In<benilla_protocol::SessionEvent>,
    index: Res<crate::net::GuidIndex>,
    mut stores: Query<&mut ObjectStore>,
) {
    let benilla_protocol::SessionEvent::PowerUpdate {
        guid,
        power_type,
        value,
    } = ev
    else {
        return;
    };
    let Some(mut store) = index.0.get(&guid).and_then(|&e| stores.get_mut(e).ok()) else {
        return;
    };
    if store
        .bypass_change_detection()
        .0
        .set_predicted_power(power_type, value)
    {
        store.set_changed();
    }
}

/// The bare unit feed, for a test driving the stock unit frames off engine state.
#[cfg(test)]
pub(crate) fn add_unit_feed(app: &mut App) {
    app.init_resource::<UnitFeedState>()
        .add_message::<crate::net::FieldChanged>()
        .add_systems(Update, feed_units);
}

/// The player state `SecureCmdOptionParse`'s `swimming` and `mounted` read, pushed each frame:
/// the swim bit of our movement flags and our mount display.
fn feed_option_state(
    script: Option<NonSendMut<UiScript>>,
    player: Option<Res<crate::player::Player>>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let state = benilla_ui::script::OptionState {
        swimming: player
            .as_deref()
            .is_some_and(|p| p.move_flags() & crate::creature_anim::move_flags::SWIMMING != 0),
        mounted: self_q
            .iter()
            .next()
            .is_some_and(|s| s.0.unit_mount_display_id() != 0),
    };
    script.set_option_state(state);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `"mouseover"` resolves through the pick the tooltip publishes: the hovered unit, until a
    /// nearer GameObject wins it and the resolver rejects the GameObject's guid.
    #[test]
    fn the_mouseover_token_resolves_nobody_behind_a_nearer_gameobject() {
        use crate::target::{Hovered, HoveredObject};
        use bevy::ecs::system::RunSystemOnce;
        const WOLF: u64 = 0xF130_0000_4500_0001;

        let mut app = App::new();
        app.init_resource::<crate::ui_party::GroupState>()
            .init_resource::<HoveredObject>();
        let wolf = app.world_mut().spawn_empty().id();
        app.insert_resource(Hovered {
            target: Some(wolf),
            guid: Some(WOLF),
            distance: 10.0,
            ..Default::default()
        });
        let resolve = |app: &mut App| {
            app.world_mut()
                .run_system_once(|tokens: UnitTokens| {
                    tokens.resolve("mouseover", &Selection::default())
                })
                .unwrap()
        };
        assert_eq!(resolve(&mut app), Some((wolf, WOLF)), "the hovered unit");
        let chest = app.world_mut().spawn_empty().id();
        app.insert_resource(HoveredObject {
            target: Some(chest),
            guid: Some(0xF110_0000_0000_0004),
            distance: 5.0,
        });
        assert_eq!(resolve(&mut app), None, "a nearer GameObject names nobody");
    }

    /// `partypetN` and `raidpetN` through the one resolver: the pet the member's descriptor names
    /// while it is held, else the roster record's, and a pet the object manager does not hold names
    /// no unit.
    mod group_pet_tokens {
        use super::*;
        use crate::net::GuidIndex;
        use crate::ui_party::{GroupState, GROUPTYPE_RAID};
        use benilla_protocol::messages::{member_status, GroupMemberEntry, PartyMemberStatsInfo};
        use benilla_protocol::ObjectFields;
        use bevy::ecs::system::RunSystemOnce;

        const ME: u64 = 0x10;
        const MY_PET: u64 = 0xF140_0000_0000_0010;
        /// `UNIT_FIELD_CHARM` and `UNIT_FIELD_SUMMON`, each a two-field guid.
        const CHARM: u16 = 6;
        const SUMMON: u16 = 8;

        /// The `i`-th party member (1-based), `0x1000 + i`, and its pet.
        fn member(i: u64) -> u64 {
            0x1000 + i
        }
        fn pet(i: u64) -> u64 {
            0xF140_0000_0000_0000 + i
        }

        fn guid_field(field: u16, guid: u64) -> [(u16, u32); 2] {
            [(field, guid as u32), (field + 1, (guid >> 32) as u32)]
        }

        fn store(pairs: &[(u16, u32)]) -> ObjectStore {
            ObjectStore(ObjectFields::from_pairs(pairs))
        }

        /// Hold `guid` in the object manager with these fields.
        fn stream(app: &mut App, guid: u64, pairs: &[(u16, u32)]) -> Entity {
            let e = app.world_mut().spawn((Guid(guid), store(pairs))).id();
            app.world_mut()
                .resource_mut::<GuidIndex>()
                .0
                .insert(guid, e);
            e
        }

        fn entry(guid: u64, flags: u8) -> GroupMemberEntry {
            GroupMemberEntry {
                name: format!("M{guid:x}"),
                guid,
                status: member_status::ONLINE,
                flags,
                roles: 0,
            }
        }

        /// Us, held, and an empty group.
        fn app() -> (App, Entity) {
            let mut app = App::new();
            app.init_resource::<GuidIndex>()
                .init_resource::<GroupState>();
            let me = app
                .world_mut()
                .spawn((SelfPlayer, Guid(ME), store(&[])))
                .id();
            app.world_mut().resource_mut::<GuidIndex>().0.insert(ME, me);
            (app, me)
        }

        /// A party of `n` with each member's record naming its pet as `online`; `pets` holds the
        /// pets, `members` the members, a member's descriptor naming its pet by `SUMMON`.
        fn party(n: u64, members: bool, pets: bool) -> App {
            let (mut app, _) = app();
            let list = (1..=n).map(|i| entry(member(i), 0)).collect();
            app.world_mut()
                .resource_mut::<GroupState>()
                .apply_list(0, 0, list, ME, None, Some(ME));
            for i in 1..=n {
                let record = PartyMemberStatsInfo {
                    status: Some(member_status::ONLINE.into()),
                    pet_guid: Some(pet(i)),
                    ..Default::default()
                };
                app.world_mut()
                    .resource_mut::<GroupState>()
                    .apply_stats(member(i), true, record);
                if pets {
                    stream(&mut app, pet(i), &[]);
                }
                if members {
                    stream(&mut app, member(i), &guid_field(SUMMON, pet(i)));
                }
            }
            app
        }

        fn resolve(app: &mut App, token: &str) -> Option<(Entity, u64)> {
            let token = token.to_string();
            app.world_mut()
                .run_system_once(move |tokens: UnitTokens| {
                    tokens.resolve(&token, &Selection::default())
                })
                .unwrap()
        }

        fn guid_of(app: &mut App, token: &str) -> Option<u64> {
            resolve(app, token).map(|(_, guid)| guid)
        }

        /// `partypetN` is the Nth party slot's pet, and `partyN` is still the slot's member: the
        /// pet arms sit ahead of `party`, whose prefix they share.
        #[test]
        fn partypet_names_the_pet_of_the_slot_and_party_the_member() {
            let mut app = party(4, true, true);
            for i in 1..=4 {
                assert_eq!(guid_of(&mut app, &format!("partypet{i}")), Some(pet(i)));
                assert_eq!(guid_of(&mut app, &format!("party{i}")), Some(member(i)));
            }
            let entity = resolve(&mut app, "partypet2").unwrap().0;
            assert_eq!(
                app.world().get::<Guid>(entity).map(|g| g.0),
                Some(pet(2)),
                "the entity is the pet's"
            );
        }

        /// The compares fold ASCII case (`_strnicmp`).
        #[test]
        fn the_pet_tokens_fold_case() {
            let mut app = party(1, true, true);
            for token in ["PartyPet1", "PARTYPET1", "partyPET1"] {
                assert_eq!(guid_of(&mut app, token), Some(pet(1)), "{token}");
            }
        }

        /// The number is 1-based and read as digits: `0`, a missing number, a letter, a sign and
        /// a number past the four slots each name no slot. The reference's `partypetN` has no bound
        /// and reads past its table; here it is nobody, as `partyN`.
        #[test]
        fn a_partypet_number_outside_one_to_four_names_nobody() {
            let mut app = party(4, true, true);
            for token in [
                "partypet0",
                "partypet5",
                "partypet",
                "partypetX",
                "partypet+1",
                "partypet1x",
                "partypet99999999999999999999",
            ] {
                assert_eq!(resolve(&mut app, token), None, "{token}");
            }
        }

        /// A held member names `CHARM` before `SUMMON`, and a held member with neither is petless
        /// whatever its record says.
        #[test]
        fn a_held_member_names_its_charm_else_summon_and_never_its_record() {
            let mut app = party(1, false, true);
            let charmed = stream(&mut app, 0xF130_0000_0000_0042, &[]);
            let held = stream(&mut app, member(1), &guid_field(SUMMON, pet(1)));
            assert_eq!(guid_of(&mut app, "partypet1"), Some(pet(1)), "SUMMON");

            let both: Vec<_> = guid_field(CHARM, 0xF130_0000_0000_0042)
                .into_iter()
                .chain(guid_field(SUMMON, pet(1)))
                .collect();
            app.world_mut().entity_mut(held).insert(store(&both));
            assert_eq!(
                resolve(&mut app, "partypet1"),
                Some((charmed, 0xF130_0000_0000_0042)),
                "CHARM first"
            );

            app.world_mut().entity_mut(held).insert(store(&[]));
            assert_eq!(
                resolve(&mut app, "partypet1"),
                None,
                "the record names a held pet, but the member's own descriptor is asked first"
            );
        }

        /// A member the object manager does not hold answers from its record's pet guid, which
        /// `partypetN` reads only while the record reads online.
        #[test]
        fn an_unheld_member_names_its_records_pet_while_online() {
            let mut app = party(1, false, false);
            assert_eq!(resolve(&mut app, "partypet1"), None, "the pet is not held");

            let held = stream(&mut app, pet(1), &[]);
            assert_eq!(resolve(&mut app, "partypet1"), Some((held, pet(1))));

            let offline = PartyMemberStatsInfo {
                status: Some(member_status::OFFLINE.into()),
                ..Default::default()
            };
            app.world_mut()
                .resource_mut::<GroupState>()
                .apply_stats(member(1), false, offline);
            assert_eq!(resolve(&mut app, "partypet1"), None, "an offline record");
        }

        /// A raid of five: us (row 1), then the wire's members, the last in another subgroup.
        fn raid() -> App {
            let (mut app, _) = app();
            let list = vec![
                entry(member(1), 0),
                entry(member(2), 1),
                entry(member(3), 1),
                entry(member(4), 2),
            ];
            app.world_mut().resource_mut::<GroupState>().apply_list(
                GROUPTYPE_RAID,
                0,
                list,
                ME,
                None,
                Some(ME),
            );
            app
        }

        /// `raidpetN` is the pet of `GetRaidRosterInfo` row N, the row `raidN` names.
        #[test]
        fn raidpet_names_the_pet_of_the_row_raid_names() {
            let mut app = raid();
            // Row 3 is `member(2)`, held with a pet; row 1 is us.
            let held_pet = stream(&mut app, pet(2), &[]);
            stream(&mut app, member(2), &guid_field(SUMMON, pet(2)));
            let mine = stream(&mut app, MY_PET, &[]);
            let me = app.world().resource::<GuidIndex>().0[&ME];
            app.world_mut()
                .entity_mut(me)
                .insert(store(&guid_field(SUMMON, MY_PET)));

            assert_eq!(resolve(&mut app, "raidpet3"), Some((held_pet, pet(2))));
            assert_eq!(
                resolve(&mut app, "raidpet1"),
                Some((mine, MY_PET)),
                "our own row"
            );
            assert_eq!(
                guid_of(&mut app, "raid3"),
                Some(member(2)),
                "raidN is unchanged"
            );
            assert_eq!(guid_of(&mut app, "raid1"), Some(ME));
            assert_eq!(resolve(&mut app, "raidpet2"), None, "row 2 has no pet");
            for token in ["raidpet0", "raidpet6", "raidpet", "raidpetX", "raidpet+3"] {
                assert_eq!(resolve(&mut app, token), None, "{token}");
            }
            assert_eq!(guid_of(&mut app, "RaidPet3"), Some(pet(2)), "case folds");
        }

        /// `raidpetN`'s record leg has no online test, where `partypetN`'s has one.
        #[test]
        fn a_raid_pet_reads_the_record_of_an_offline_member_a_party_pet_does_not() {
            let mut app = raid();
            let record = PartyMemberStatsInfo {
                status: Some(member_status::OFFLINE.into()),
                pet_guid: Some(pet(1)),
                ..Default::default()
            };
            app.world_mut()
                .resource_mut::<GroupState>()
                .apply_stats(member(1), true, record);
            let held = stream(&mut app, pet(1), &[]);
            // Row 2 and party slot 1 are the same member.
            assert_eq!(resolve(&mut app, "raidpet2"), Some((held, pet(1))));
            assert_eq!(resolve(&mut app, "partypet1"), None);
        }

        /// A party pet in view gets a distance, so the range verbs answer for it: the feed measures
        /// the held units the resolver's inputs list, and the VM resolves the token to one of them.
        #[test]
        fn the_reach_feed_measures_a_party_pet() {
            let mut app = party(1, true, false);
            let me = app.world().resource::<GuidIndex>().0[&ME];
            app.world_mut()
                .entity_mut(me)
                .insert(Transform::from_xyz(0.0, 0.0, 0.0));
            let held = stream(&mut app, pet(1), &[]);
            // Along one axis, so d² is exactly yards².
            app.world_mut()
                .entity_mut(held)
                .insert(Transform::from_xyz(15.0, 0.0, 0.0));
            app.init_resource::<Reputations>();
            app.insert_resource(Selection::default());
            app.insert_non_send_resource(UiScript::new().unwrap());
            app.world_mut()
                .non_send_resource_mut::<UiScript>()
                .set_unit_guids(&benilla_ui::script::UnitGuids {
                    player: ME,
                    party: [member(1), 0, 0, 0],
                    party_pets: [pet(1), 0, 0, 0],
                    held: HashMap::from([(ME, 0), (pet(1), 0)]),
                    ..Default::default()
                });
            app.world_mut().run_system_once(feed_unit_reach).unwrap();
            let answer = |app: &App, expr: &str| {
                app.world()
                    .non_send_resource::<UiScript>()
                    .eval::<bool>(expr)
                    .unwrap()
            };
            assert!(answer(
                &app,
                r#"return CheckInteractDistance("partypet1", 4) ~= nil"#
            ));
            assert!(
                answer(
                    &app,
                    r#"return CheckInteractDistance("partypet1", 1) == nil"#
                ),
                "15 yards is outside the 10-yard row"
            );
            assert!(
                answer(
                    &app,
                    r#"return CheckInteractDistance("partypet2", 4) == nil"#
                ),
                "no second member"
            );
        }
    }

    /// The `target` chain through the one resolver: a base, then a hop off each held unit's
    /// `UNIT_FIELD_TARGET`, so `party1target`, `pettarget` and `raid3target` name the unit at the
    /// end of the chain and any break in it names nobody.
    mod target_chain_tokens {
        use super::*;
        use crate::net::GuidIndex;
        use crate::target::{Hovered, HoveredObject};
        use crate::ui_party::{GroupState, GROUPTYPE_RAID};
        use benilla_protocol::messages::{member_status, GroupMemberEntry};
        use benilla_protocol::ObjectFields;
        use bevy::ecs::system::RunSystemOnce;

        const ME: u64 = 0x10;
        /// party1 and raid2, party2 and raid3, party3 and raid4.
        const A: u64 = 0x1001;
        const B: u64 = 0x1002;
        const C: u64 = 0x1003;
        const MOB: u64 = 0xF130_0000_0000_0001;
        /// A unit with no target, the mouseover.
        const LONELY: u64 = 0xF130_0000_0000_0002;
        const MY_PET: u64 = 0xF140_0000_0000_0010;
        const A_PET: u64 = 0xF140_0000_0000_1001;
        /// A game object that still carries a value where a unit keeps its target.
        const CHEST: u64 = 0xF110_0000_0000_0003;
        /// Named by `C`'s target and held by no one.
        const GONE: u64 = 0xF130_0000_0000_0099;

        /// `OBJECT_FIELD_TYPE`, and the masks the object manager's typemask test reads.
        const OBJECT_TYPE: u16 = 2;
        const PLAYER: u32 = 0x19;
        const UNIT: u32 = 0x09;
        const GAME_OBJECT: u32 = 0x21;
        /// `UNIT_FIELD_SUMMON` and `UNIT_FIELD_TARGET`, each a two-field guid.
        const SUMMON: u16 = 8;
        const TARGET: u16 = 16;

        fn fields(kind: u32, summon: u64, target: u64) -> ObjectStore {
            let mut pairs = vec![(OBJECT_TYPE, kind)];
            for (field, guid) in [(SUMMON, summon), (TARGET, target)] {
                pairs.push((field, guid as u32));
                pairs.push((field + 1, (guid >> 32) as u32));
            }
            ObjectStore(ObjectFields::from_pairs(&pairs))
        }

        fn entry(guid: u64) -> GroupMemberEntry {
            GroupMemberEntry {
                name: format!("M{guid:x}"),
                guid,
                status: member_status::ONLINE,
                flags: 0,
                roles: 0,
            }
        }

        /// The world's targets: we target the mob and so does `A`, the mob targets `B`, `B` targets
        /// `A`, our pet targets the mob and `A`'s targets `B`, `C` targets a unit nobody holds, and
        /// `LONELY` targets nobody. We select the mob and hover `LONELY`. The group is a raid of
        /// us, `A`, `B` and `C`, all in our subgroup, so the three are also party slots 1 to 3.
        fn app() -> App {
            let mut app = App::new();
            app.init_resource::<GuidIndex>()
                .init_resource::<GroupState>()
                .init_resource::<HoveredObject>();
            let spawn = |app: &mut App, guid: u64, store: ObjectStore| {
                let e = app.world_mut().spawn((Guid(guid), store)).id();
                app.world_mut()
                    .resource_mut::<GuidIndex>()
                    .0
                    .insert(guid, e);
                e
            };
            let me = spawn(&mut app, ME, fields(PLAYER, MY_PET, MOB));
            app.world_mut().entity_mut(me).insert(SelfPlayer);
            spawn(&mut app, A, fields(PLAYER, A_PET, MOB));
            spawn(&mut app, B, fields(PLAYER, 0, A));
            spawn(&mut app, C, fields(PLAYER, 0, GONE));
            let mob = spawn(&mut app, MOB, fields(UNIT, 0, B));
            let lonely = spawn(&mut app, LONELY, fields(UNIT, 0, 0));
            spawn(&mut app, MY_PET, fields(UNIT, 0, MOB));
            spawn(&mut app, A_PET, fields(UNIT, 0, B));
            app.world_mut().resource_mut::<GroupState>().apply_list(
                GROUPTYPE_RAID,
                0,
                vec![entry(A), entry(B), entry(C)],
                ME,
                None,
                Some(ME),
            );
            app.insert_resource(Selection {
                target: Some(mob),
                guid: Some(MOB),
                ..Default::default()
            });
            app.insert_resource(Hovered {
                target: Some(lonely),
                guid: Some(LONELY),
                distance: 5.0,
                ..Default::default()
            });
            let mut bar = crate::ui_pet::PetBar::default();
            bar.spells.pet_guid = MY_PET;
            app.insert_resource(bar);
            app
        }

        fn resolve(app: &mut App, token: &str) -> Option<(Entity, u64)> {
            let token = token.to_string();
            app.world_mut()
                .run_system_once(move |tokens: UnitTokens, selection: Res<Selection>| {
                    tokens.resolve(&token, &selection)
                })
                .unwrap()
        }

        fn guid_of(app: &mut App, token: &str) -> Option<u64> {
            resolve(app, token).map(|(_, guid)| guid)
        }

        /// Every base takes hops, and each hop reads the current unit's own target, so the answer
        /// is the unit the chain ends on, its entity the one the object manager holds.
        #[test]
        fn a_target_suffix_names_the_unit_at_the_end_of_the_chain() {
            let mut app = app();
            for (token, want) in [
                ("party1target", MOB),
                ("party2target", A),
                ("party1targettarget", B),
                ("party1targettargettarget", A),
                ("playertarget", MOB),
                ("playertargettarget", B),
                ("partypet1target", B),
                ("pettarget", MOB),
                ("pettargettarget", B),
                ("target", MOB),
                ("targettarget", B),
                ("targettargettarget", A),
                ("raid1target", MOB),
                ("raid2target", MOB),
                ("raid3target", A),
                ("raid2targettarget", B),
                ("raidpet1target", MOB),
                ("raidpet2target", B),
                ("mouseover", LONELY),
            ] {
                assert_eq!(guid_of(&mut app, token), Some(want), "{token}");
            }
            let (entity, guid) = resolve(&mut app, "party2targettarget").unwrap();
            assert_eq!(guid, MOB);
            assert_eq!(
                app.world().get::<Guid>(entity).map(|g| g.0),
                Some(MOB),
                "the entity is the mob's"
            );
        }

        /// A unit with no target ends the chain, as does a target the object manager does not hold,
        /// a hop off anything but a unit (typemask 8) and a base that names no one.
        #[test]
        fn a_chain_that_breaks_names_nobody() {
            let mut app = app();
            for token in [
                // The mouseover targets nobody, and so nothing follows it.
                "mouseovertarget",
                "mouseovertargettarget",
                // `C` targets a guid nobody holds.
                "party3target",
                "raid4target",
                // Nor does a further hop go past the break.
                "party3targettarget",
                // A base no one holds: party4 is nobody, raid5 is past the roster, a missing or
                // zero number names no row, and `B` has no pet.
                "party4target",
                "raid5target",
                "raid0target",
                "partytarget",
                "partypet2target",
                "raidpet3target",
            ] {
                assert_eq!(resolve(&mut app, token), None, "{token}");
            }

            // A hop off a game object is nobody, whatever its descriptor holds where a unit
            // keeps its target.
            let chest = app
                .world_mut()
                .spawn((Guid(CHEST), fields(GAME_OBJECT, 0, A)))
                .id();
            app.world_mut()
                .resource_mut::<GuidIndex>()
                .0
                .insert(CHEST, chest);
            app.insert_resource(Selection {
                target: Some(chest),
                guid: Some(CHEST),
                ..Default::default()
            });
            assert_eq!(guid_of(&mut app, "target"), Some(CHEST), "the base is held");
            assert_eq!(resolve(&mut app, "targettarget"), None);
        }

        /// Text after the base, or after a hop, that is not `target` names nobody, and `npc` is
        /// an exact compare with no chain.
        #[test]
        fn text_that_is_not_a_target_hop_names_nobody() {
            let mut app = app();
            for token in [
                "party1foo",
                "party1targetfoo",
                "party1targettarge",
                "party1 target",
                "playerfoo",
                "playertargets",
                "pettarge",
                "targetfoo",
                "mouseoverx",
                "raid3targettargetfoo",
                "npctarget",
                "bogus",
                "",
            ] {
                assert_eq!(resolve(&mut app, token), None, "{token}");
            }
            // The control: the same tokens without the tail resolve.
            assert_eq!(guid_of(&mut app, "party1"), Some(A));
            assert_eq!(guid_of(&mut app, "player"), Some(ME));
        }

        /// The compares fold ASCII case, the hops' included (`_strnicmp`), and the chain has no
        /// depth limit: three hops round `A`, the mob and `B` come back to `A`.
        #[test]
        fn a_chain_folds_case_and_has_no_depth_limit() {
            let mut app = app();
            for token in [
                "PARTY1TARGET",
                "Party1Target",
                "party1TARGET",
                "PartY1tArGeT",
            ] {
                assert_eq!(guid_of(&mut app, token), Some(MOB), "{token}");
            }
            assert_eq!(guid_of(&mut app, "PlayerTarget"), Some(MOB));
            assert_eq!(guid_of(&mut app, "TargetTargetTarget"), Some(A));
            assert_eq!(guid_of(&mut app, "RaidPet1Target"), Some(MOB));
            for hops in [3, 30, 3000] {
                let token = format!("party1{}", "target".repeat(hops));
                assert_eq!(guid_of(&mut app, &token), Some(A), "{hops} hops round A");
            }
            let token = format!("party1{}", "target".repeat(3001));
            assert_eq!(guid_of(&mut app, &token), Some(MOB));
        }

        /// A number wraps in 32 bits as the reference's inline parse does: 2^32 + 1 is row 1, and
        /// digits stop at the first letter.
        #[test]
        fn a_row_number_wraps_as_the_reference_parse_does() {
            let mut app = app();
            assert_eq!(guid_of(&mut app, "party4294967297target"), Some(MOB));
            assert_eq!(guid_of(&mut app, "party01target"), Some(MOB));
            assert_eq!(guid_of(&mut app, "party1x"), None);
        }
    }

    /// The range verbs end to end, with the aura feed that pushes the resolver's inputs and the
    /// reach feed that measures the units they hold: `CheckInteractDistance` and `CanInspect`
    /// answer for a `target` chain (the reference's two resolve their token through the one
    /// unit-token resolver), on the unit the chain names and not on the base.
    #[test]
    fn the_range_verbs_answer_for_a_target_chain() {
        use benilla_protocol::messages::{member_status, GroupMemberEntry, ObjectFields};

        const ME: u64 = 0x10;
        const A: u64 = 0x1001;
        const B: u64 = 0x1002;
        const MOB: u64 = 0xF130_0000_0000_0001;
        const A_PET: u64 = 0xF140_0000_0000_1001;
        /// `OBJECT_FIELD_TYPE`, `UNIT_FIELD_SUMMON`, `UNIT_FIELD_TARGET` and `UNIT_FIELD_FLAGS`.
        const OBJECT_TYPE: u16 = 2;
        const SUMMON: u16 = 8;
        const TARGET: u16 = 16;
        const FLAGS: u16 = 46;
        /// `UNIT_FIELD_FLAGS` bit 1 (NON_ATTACKABLE), which `can_attack` refuses.
        const NON_ATTACKABLE: u32 = 1 << 1;

        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::state::app::StatesPlugin))
            .insert_state(crate::char_select::ClientState::InWorld)
            .init_resource::<Selection>()
            .init_resource::<crate::ui_pet::PetBar>()
            .init_resource::<crate::net::GuidIndex>()
            .init_resource::<Reputations>()
            .init_resource::<crate::ui_party::GroupState>()
            .insert_resource(NetCommands(tx))
            .add_plugins(crate::ui_aura::UiAuraPlugin)
            .add_systems(Update, feed_unit_reach.after(crate::ui_aura::AuraEvents));
        app.insert_non_send_resource(UiScript::new().unwrap());

        // We at the origin, party1 (`A`) 5 yards off with a pet 15 yards off, the mob 20 yards
        // off, and `B` 3 yards off. We and `A` target the mob, the mob targets `B`, the pet
        // targets `B`, and `B` targets nobody. The players are not attackable, so inspectable.
        let mut spawn = |guid: u64, kind: u32, summon: u64, target: u64, yards: f32| {
            let mut pairs = vec![(OBJECT_TYPE, kind), (FLAGS, NON_ATTACKABLE)];
            for (field, guid) in [(SUMMON, summon), (TARGET, target)] {
                pairs.push((field, guid as u32));
                pairs.push((field + 1, (guid >> 32) as u32));
            }
            let e = app
                .world_mut()
                .spawn((
                    Guid(guid),
                    ObjectStore(ObjectFields::from_pairs(&pairs)),
                    // Along one axis, so d² is exactly yards².
                    Transform::from_xyz(yards, 0.0, 0.0),
                ))
                .id();
            app.world_mut()
                .resource_mut::<crate::net::GuidIndex>()
                .0
                .insert(guid, e);
            e
        };
        let me = spawn(ME, 0x19, 0, MOB, 0.0);
        spawn(A, 0x19, A_PET, MOB, 5.0);
        spawn(A_PET, 0x09, 0, B, 15.0);
        let mob = spawn(MOB, 0x09, 0, B, 20.0);
        spawn(B, 0x19, 0, 0, 3.0);
        app.world_mut().entity_mut(me).insert(SelfPlayer);
        app.world_mut()
            .resource_mut::<crate::ui_party::GroupState>()
            .apply_list(
                0,
                0,
                vec![GroupMemberEntry {
                    name: "Brisca".into(),
                    guid: A,
                    status: member_status::ONLINE,
                    flags: 0,
                    roles: 0,
                }],
                ME,
                None,
                Some(ME),
            );
        app.insert_resource(Selection {
            target: Some(mob),
            guid: Some(MOB),
            ..Default::default()
        });
        app.update();

        let answers = |app: &App, expr: &str| {
            app.world()
                .non_send_resource::<UiScript>()
                .eval::<bool>(&format!("return {expr} ~= nil"))
                .unwrap()
        };
        // The base and each hop, at their own distances: the 10-yard row is `1`, the 30-yard `4`.
        for (token, kind, want) in [
            ("player", 1, true),
            ("target", 1, false),
            ("target", 4, true),
            ("playertarget", 1, false),
            ("playertarget", 4, true),
            ("party1target", 1, false),
            ("party1target", 4, true),
            ("PARTY1TARGET", 4, true),
            ("party1targettarget", 1, true),
            ("targettarget", 1, true),
            ("partypet1", 1, false),
            ("partypet1", 4, true),
            ("partypet1target", 1, true),
            // `B` targets nobody, so the chain ends there.
            ("party1targettargettarget", 4, false),
            ("party2target", 4, false),
            ("party1foo", 4, false),
            ("npctarget", 4, false),
        ] {
            assert_eq!(
                answers(
                    &app,
                    &format!(r#"CheckInteractDistance("{token}", {kind})"#)
                ),
                want,
                "CheckInteractDistance({token}, {kind})"
            );
        }
        // `CanInspect`: a player within 10 yards, so `B` and not the mob 20 yards off.
        for (token, want) in [
            ("party1targettarget", true),
            ("targettarget", true),
            ("party1", true),
            ("party1target", false),
            ("target", false),
            ("party1targettargettarget", false),
        ] {
            assert_eq!(
                answers(&app, &format!(r#"CanInspect("{token}")"#)),
                want,
                "CanInspect({token})"
            );
        }
    }

    /// The reach feed measures the guids the aura feed pushes, so a frame's feeds must run in that
    /// order: read off the declared graph, the aura feed's set comes before the reach feed.
    #[test]
    fn the_reach_feed_runs_after_the_aura_feed_it_reads() {
        use crate::game_plugins::schedule_tests::{census, headless_client, SyncPoints};

        let mut app = headless_client();
        let c = census(&mut app, Update, SyncPoints::Declared);
        if !c.systems.values().any(|s| s.name.contains("benilla_app::")) {
            eprintln!("skipped: this build carries no type names");
            return;
        }
        let one = |suffix: &str| {
            let mut hits = c.systems.iter().filter(|(_, s)| s.name.ends_with(suffix));
            match (hits.next(), hits.next()) {
                (Some((k, _)), None) => *k,
                _ => panic!("exactly one system named `…{suffix}` in Update"),
            }
        };
        let aura = one("::ui_aura::feed_auras");
        let reach = one("::ui_unit::feed_unit_reach");
        assert!(
            c.dependencies.contains(&(aura, reach)),
            "feed_unit_reach is declared after feed_auras"
        );
    }

    /// The team digit comes off the race byte, whatever template sits beside it (a GM's 35).
    #[test]
    fn the_team_digit_comes_off_the_race_byte_not_the_faction_template() {
        use benilla_protocol::ObjectFields;
        /// `UNIT_FIELD_FACTIONTEMPLATE` and `UNIT_FIELD_BYTES_0` (`[obj+0x110]+0x74`, `+0x78`).
        const FACTIONTEMPLATE: u16 = 35;
        const BYTES_0: u16 = 36;
        /// vmangos's GM template: `FactionTemplate.dbc` group mask 0.
        const GM_TEMPLATE: u32 = 35;

        let team = |fields: &[(u16, u32)]| {
            snapshot(
                &ObjectStore(ObjectFields::from_pairs(fields)),
                0,
                None,
                0,
                None,
                Default::default(),
            )
            .pvp_team
        };
        // Byte 0 of BYTES_0 is the race; the class in byte 1 must not disturb it.
        let human_warrior = 1 | (1 << 8);
        let scourge_mage = 5 | (8 << 8);
        assert_eq!(team(&[(BYTES_0, human_warrior)]), 1, "Human → Alliance");
        assert_eq!(team(&[(BYTES_0, scourge_mage)]), 0, "Scourge → Horde");
        assert_eq!(
            team(&[(BYTES_0, human_warrior), (FACTIONTEMPLATE, GM_TEMPLATE)]),
            1,
            "a GM keeps his race's side"
        );
        assert_eq!(
            team(&[(BYTES_0, scourge_mage), (FACTIONTEMPLATE, GM_TEMPLATE)]),
            0,
            "…on both sides"
        );
        // No race byte is the engine's bounds-failure −1, and a template cannot stand in for it.
        assert_eq!(team(&[]), -1, "no race byte, no team digit");
        assert_eq!(
            team(&[(FACTIONTEMPLATE, 1)]),
            -1,
            "and a template is not one"
        );
    }

    /// A new VM is seeded with the shipped `Languages.dbc`, so each race's own tongue resolves by
    /// name to the id vmangos keys `KnowsLanguage` on (`SharedDefines.h:253-269`).
    #[test]
    fn a_new_vm_resolves_every_racial_language_by_name() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let mut world = World::new();
        world.insert_resource(LanguagesRes(
            benilla_formats::load_languages(&mut chain).expect("Languages.dbc"),
        ));
        let mut script = UiScript::new().expect("VM");
        seed_language_table(&world, &mut script);
        let want = [
            ("Orcish", 1),
            ("Darnassian", 2),
            ("Taurahe", 3),
            ("Dwarvish", 6),
            ("Common", 7),
            ("Gnomish", 13),
            ("Troll", 14),
            ("Gutterspeak", 33),
        ];
        for (name, _) in want {
            script
                .run(&format!(r#"SendChatMessage("hi", "SAY", "{name}")"#))
                .unwrap_or_else(|e| panic!("{name}: {e}"));
        }
        let got: Vec<Option<u32>> = script
            .take_chat_sends()
            .iter()
            .map(|c| c.language)
            .collect();
        let ids: Vec<Option<u32>> = want.iter().map(|&(_, id)| Some(id)).collect();
        assert_eq!(got, ids);
    }

    /// [`race_pvp_team`] against the shipped DBCs, walked as the reference walks them.
    #[test]
    #[ignore = "reads 1.12.1 data; port to the 3.3.5a install (#51)"]
    fn race_pvp_team_matches_the_shipped_tables() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let want = benilla_formats::load_race_pvp_teams(&mut chain).expect("ChrRaces walk");
        // 1.12 ships nine rows; a truncated map would pass by asserting nothing.
        assert_eq!(want.len(), 9, "ChrRaces.dbc row count");
        for (&race, &team) in &want {
            assert_eq!(race_pvp_team(race), team, "race {race}");
        }
        assert!(want.values().any(|&t| t == 0), "some race is Horde");
        assert!(want.values().any(|&t| t == 1), "some race is Alliance");
        for race in [0u8, 10, 255] {
            assert!(!want.contains_key(&race));
            assert_eq!(race_pvp_team(race), -1, "race {race} has no ChrRaces row");
        }
    }

    /// A unit's first snapshot is no transition: the reference's create runs no notify pass.
    #[test]
    fn a_flags_change_fires_unit_flags_with_the_token() {
        let fired = |prev: Option<UnitState>, cur: UnitState, edges: &FieldEdges| -> Vec<String> {
            let mut s = UiScript::new().unwrap();
            s.run(
                r#"
                SEEN = {}
                local f = CreateFrame("Frame")
                f:RegisterEvent("UNIT_FLAGS")
                f:SetScript("OnEvent", function(self, event, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9) table.insert(SEEN, event .. ":" .. arg1) end)
            "#,
            )
            .unwrap();
            fire_transitions(&mut s, "pet", prev.as_ref(), &cur, edges);
            s.eval::<Vec<String>>("return SEEN").unwrap()
        };
        const PET: u64 = 0xF140_0000_0000_0001;
        let base = UnitState {
            exists: true,
            has_object: true,
            guid: PET,
            flags: 0x8,
            ..Default::default()
        };
        let moved = FieldEdges::of(&[(PET, benilla_protocol::field::FIELD_UNIT_FLAGS)]);
        assert_eq!(
            fired(
                Some(base.clone()),
                UnitState {
                    flags: 0x8 | 0x0400_0000,
                    ..base.clone()
                },
                &moved,
            ),
            vec!["UNIT_FLAGS:pet".to_string()]
        );
        assert_eq!(
            fired(Some(base.clone()), base.clone(), &FieldEdges::default()),
            Vec::<String>::new()
        );
        assert_eq!(
            fired(None, base.clone(), &FieldEdges::default()),
            Vec::<String>::new()
        );
        // The unit's edge, not the token's history: a token acquired as the field moved hears it
        // (the reference fans out at notify time), and another unit's edge does not.
        assert_eq!(
            fired(None, base.clone(), &moved),
            vec!["UNIT_FLAGS:pet".to_string()]
        );
        assert_eq!(
            fired(
                Some(base.clone()),
                base.clone(),
                &FieldEdges::of(&[(PET + 1, benilla_protocol::field::FIELD_UNIT_FLAGS)]),
            ),
            Vec::<String>::new()
        );
    }

    /// LOST going down, GAINED going up (boot is in control); far sight on every change.
    #[test]
    fn the_control_and_far_sight_edges_fire_once_each_way() {
        use bevy::prelude::*;
        const FIELD_PLAYER_FARSIGHT: u16 = 712;
        let mut app = App::new();
        app.init_resource::<crate::player::Player>()
            .add_systems(Update, (feed_player_control, feed_farsight_focus));
        let script = UiScript::new().unwrap();
        script
            .run(
                r#"
                SEEN = {}
                local f = CreateFrame("Frame")
                f:RegisterEvent("PLAYER_CONTROL_LOST")
                f:RegisterEvent("PLAYER_CONTROL_GAINED")
                f:RegisterEvent("PLAYER_FARSIGHT_FOCUS_CHANGED")
                f:SetScript("OnEvent", function() table.insert(SEEN, event) end)
            "#,
            )
            .unwrap();
        app.insert_non_send_resource(script);
        let me = app
            .world_mut()
            .spawn((
                SelfPlayer,
                Guid(0x77),
                ObjectStore(benilla_protocol::ObjectFields::default()),
            ))
            .id();
        let seen = |app: &mut App| -> Vec<String> {
            app.update();
            let mut s = app.world_mut().non_send_resource_mut::<UiScript>();
            s.resolve();
            let out = s.eval::<Vec<String>>("return SEEN").unwrap();
            s.run("SEEN = {}").unwrap();
            out
        };
        assert_eq!(
            seen(&mut app),
            Vec::<String>::new(),
            "in control, no far sight: quiet"
        );
        app.world_mut()
            .resource_mut::<crate::player::Player>()
            .control_lost = true;
        assert_eq!(seen(&mut app), vec!["PLAYER_CONTROL_LOST".to_string()]);
        assert_eq!(seen(&mut app), Vec::<String>::new(), "held, not repeated");
        app.world_mut()
            .resource_mut::<crate::player::Player>()
            .control_lost = false;
        assert_eq!(seen(&mut app), vec!["PLAYER_CONTROL_GAINED".to_string()]);

        // A world entry hands control back with no event (`0x513880`), the flag re-seeded.
        app.world_mut()
            .resource_mut::<crate::player::Player>()
            .control_lost = true;
        assert_eq!(seen(&mut app), vec!["PLAYER_CONTROL_LOST".to_string()]);
        {
            let mut p = app.world_mut().resource_mut::<crate::player::Player>();
            p.control_lost = false;
            p.control_inits += 1;
        }
        assert_eq!(
            seen(&mut app),
            Vec::<String>::new(),
            "no GAINED at a world entry"
        );
        assert!(app
            .world_mut()
            .non_send_resource_mut::<UiScript>()
            .eval::<bool>("return HasFullControl() == 1")
            .unwrap());

        let set_farsight = |app: &mut App, guid: u64| {
            app.world_mut().entity_mut(me).insert(ObjectStore(
                benilla_protocol::ObjectFields::from_pairs(&[
                    (FIELD_PLAYER_FARSIGHT, guid as u32),
                    (FIELD_PLAYER_FARSIGHT + 1, (guid >> 32) as u32),
                ]),
            ));
        };
        set_farsight(&mut app, 0xf130_0000_0000_0042);
        assert_eq!(
            seen(&mut app),
            vec!["PLAYER_FARSIGHT_FOCUS_CHANGED".to_string()],
            "set — whether or not the guid resolves"
        );
        assert_eq!(seen(&mut app), Vec::<String>::new());
        set_farsight(&mut app, 0);
        assert_eq!(
            seen(&mut app),
            vec!["PLAYER_FARSIGHT_FOCUS_CHANGED".to_string()],
            "cleared — the other leg"
        );
    }

    #[test]
    fn the_tapped_bit_fires_unit_faction_and_the_by_player_bit_fires_nothing() {
        let fired = |prev: UnitState, cur: UnitState| -> Vec<String> {
            let mut s = UiScript::new().unwrap();
            s.run(
                r#"
                SEEN = {}
                local f = CreateFrame("Frame")
                f:RegisterEvent("UNIT_FACTION")
                f:SetScript("OnEvent", function() table.insert(SEEN, event) end)
            "#,
            )
            .unwrap();
            fire_transitions(&mut s, "target", Some(&prev), &cur, &FieldEdges::default());
            s.eval::<Vec<String>>("return SEEN").unwrap()
        };
        let base = UnitState {
            exists: true,
            has_object: true,
            ..Default::default()
        };

        // The `0x4` edge fires `UNIT_FACTION`, the reference's event 29.
        assert_eq!(
            fired(
                base.clone(),
                UnitState {
                    tapped: true,
                    ..base.clone()
                }
            ),
            vec!["UNIT_FACTION".to_string()],
            "a unit becoming tapped must repaint the frames that draw it"
        );
        assert_eq!(
            fired(
                UnitState {
                    tapped: true,
                    ..base.clone()
                },
                base.clone()
            ),
            vec!["UNIT_FACTION".to_string()],
        );
        // The `0x8` edge fires nothing: the reference's watcher has no arm for it.
        assert!(
            fired(
                UnitState {
                    tapped: true,
                    ..base.clone()
                },
                UnitState {
                    tapped: true,
                    tapped_by_player: true,
                    ..base.clone()
                }
            )
            .is_empty(),
            "bit 0x8 has no delta arm — firing on it would be an invention"
        );
        // The control against firing always.
        assert!(fired(base.clone(), base.clone()).is_empty());
    }

    /// The control is a bit no `UnitState` field decodes, so the trigger is the raw dword.
    #[test]
    fn player_flags_changed_fires_per_token_on_any_bit_and_never_on_first_sight() {
        let fired = |prev: Option<UnitState>, cur: UnitState, edges: &FieldEdges| -> Vec<String> {
            let mut s = UiScript::new().unwrap();
            s.run(
                r#"
                SEEN = {}
                local f = CreateFrame("Frame")
                f:RegisterEvent("PLAYER_FLAGS_CHANGED")
                f:SetScript("OnEvent", function(self, event, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9) table.insert(SEEN, event .. ":" .. arg1) end)
            "#,
            )
            .unwrap();
            fire_transitions(&mut s, "target", prev.as_ref(), &cur, edges);
            s.eval::<Vec<String>>("return SEEN").unwrap()
        };
        const THEM: u64 = 0x2a;
        let base = UnitState {
            exists: true,
            has_object: true,
            guid: THEM,
            ..Default::default()
        };
        let with = |flags: u32| UnitState {
            player_flags: flags,
            group_leader: flags & 0x1 != 0,
            ghost: flags & 0x10 != 0,
            ..base.clone()
        };
        let moved = FieldEdges::of(&[(THEM, benilla_protocol::field::FIELD_PLAYER_FLAGS)]);
        let still = FieldEdges::default();

        // `PLAYER_FLAGS_GROUP_LEADER`, the bit the 1.12 target frame reads.
        assert_eq!(
            fired(Some(with(0)), with(0x1), &moved),
            vec!["PLAYER_FLAGS_CHANGED:target".to_string()],
            "arg1 is the unit token, and there is no arg2 (\"%s\", token))"
        );
        // And back down: the reference tests the XOR diff, not the new value.
        assert_eq!(
            fired(Some(with(0x1)), with(0), &moved),
            vec!["PLAYER_FLAGS_CHANGED:target".to_string()]
        );

        // `PLAYER_FLAGS_HIDE_HELM`: no `UnitState` field decodes it, and the reference's
        // `PLAYER_FLAGS_CHANGED` watch tests no bit.
        assert_eq!(
            fired(Some(with(0)), with(0x400), &moved),
            vec!["PLAYER_FLAGS_CHANGED:target".to_string()],
            "an undecoded bit still fires it — the handler tests no bit at all"
        );

        assert!(
            fired(None, with(0x1), &still).is_empty(),
            "a unit's first snapshot is its create, not a transition"
        );
        // The control against firing always.
        assert!(fired(Some(with(0x1)), with(0x1), &still).is_empty());
    }

    /// Event 137 watches descriptor field 143 over one dword (`repe cmpsb`), so any bit fires it;
    /// the control is `0x2` (`UNIT_DYNFLAG_TRACK_UNIT`), which no `UnitState` field decodes.
    #[test]
    fn unit_dynamic_flags_fires_per_token_on_any_bit_and_never_on_first_sight() {
        let fired = |prev: Option<UnitState>, cur: UnitState, edges: &FieldEdges| -> Vec<String> {
            let mut s = UiScript::new().unwrap();
            s.run(
                r#"
                SEEN = {}
                local f = CreateFrame("Frame")
                f:RegisterEvent("UNIT_DYNAMIC_FLAGS")
                f:SetScript("OnEvent", function(self, event, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9) table.insert(SEEN, event .. ":" .. arg1) end)
            "#,
            )
            .unwrap();
            fire_transitions(&mut s, "target", prev.as_ref(), &cur, edges);
            s.eval::<Vec<String>>("return SEEN").unwrap()
        };
        const MOB: u64 = 0xF130_0000_0000_0007;
        let moved = FieldEdges::of(&[(MOB, benilla_protocol::field::FIELD_UNIT_DYNAMIC_FLAGS)]);
        let still = FieldEdges::default();
        let with = |dyn_flags: u32| UnitState {
            exists: true,
            has_object: true,
            guid: MOB,
            dynamic_flags: dyn_flags,
            tapped: dyn_flags & 0x4 != 0,
            tapped_by_player: dyn_flags & 0x8 != 0,
            ..Default::default()
        };

        // `0x4` TAPPED, the grey-bar state addons register this event for.
        assert_eq!(
            fired(Some(with(0)), with(0x4), &moved),
            vec!["UNIT_DYNAMIC_FLAGS:target".to_string()],
            "arg1 is the unit token, and there is no arg2"
        );
        assert_eq!(
            fired(Some(with(0)), with(0x2), &moved),
            vec!["UNIT_DYNAMIC_FLAGS:target".to_string()],
            "an undecoded bit still fires it — the watch is a memcmp over the dword"
        );
        // The create runs no notify pass.
        assert!(fired(None, with(0x4), &still).is_empty());
        // The control against firing always.
        assert!(fired(Some(with(0x4)), with(0x4), &still).is_empty());
    }

    /// `PLAYER_UPDATE_RESTING` fires, inside the `0x20` arm whose `je` skips.
    #[test]
    fn the_self_flag_events_each_fire_on_their_own_bits() {
        use bevy::ecs::system::RunSystemOnce;
        const FIELD_PLAYER_FLAGS: u16 = 190;

        let mut app = App::new();
        app.add_message::<FieldChanged>();
        app.init_resource::<Selection>()
            .init_resource::<UnitFeedState>()
            .init_resource::<NameCache>()
            .init_resource::<Reputations>()
            .init_resource::<crate::ui_party::GroupState>()
            .init_resource::<crate::ui_chat::ChatLog>()
            // The feed's `MessageSink` reads chat and sounds: a message key's row carries its cue.
            .init_resource::<crate::sound::MessageSounds>()
            .init_resource::<crate::ui_guild::GuildState>();
        let (tx, _rx) = crossbeam_channel::unbounded();
        app.insert_resource(NetCommands(tx));
        let script = UiScript::new().unwrap();
        script
            .run(
                r#"
                SEEN = {}
                local f = CreateFrame("Frame")
                f:RegisterEvent("PLAYER_UPDATE_RESTING")
                f:RegisterEvent("PLAYTIME_CHANGED")
                f:RegisterEvent("UPDATE_EXHAUSTION")
                f:SetScript("OnEvent", function() table.insert(SEEN, event) end)
            "#,
            )
            .unwrap();
        app.insert_non_send_resource(script);
        let me = app
            .world_mut()
            .spawn((
                SelfPlayer,
                Guid(0x77),
                ObjectStore(
                    benilla_protocol::ObjectFields::from_pairs(&[])
                        .into_created(benilla_protocol::messages::ObjectType::Player),
                ),
            ))
            .id();

        // Set PLAYER_FLAGS, run the feed, read back what fired.
        let step = |app: &mut App, flags: u32| -> Vec<String> {
            app.world_mut()
                .entity_mut(me)
                .get_mut::<ObjectStore>()
                .unwrap()
                .0
                .merge(benilla_protocol::ObjectFields::from_pairs(&[(
                    FIELD_PLAYER_FLAGS,
                    flags,
                )]));
            app.world_mut().run_system_once(feed_units).unwrap();
            let mut s = app.world_mut().non_send_resource_mut::<UiScript>();
            s.resolve();
            let out = s.eval::<Vec<String>>("return SEEN").unwrap();
            s.run("SEEN = {}").unwrap();
            out
        };

        // `UPDATE_EXHAUSTION` is other watchers and fires on first sight here, so it is filtered
        // out.
        let flag_events = |v: Vec<String>| -> Vec<String> {
            v.into_iter().filter(|e| e != "UPDATE_EXHAUSTION").collect()
        };
        assert!(
            flag_events(step(&mut app, 0)).is_empty(),
            "the login descriptor is a create: structurally silent"
        );

        // `PLAYER_FLAGS_HIDE_HELM` `0x400` moves and the resting bit does not.
        assert!(
            flag_events(step(&mut app, 0x400)).is_empty(),
            "a non-resting, non-playtime bit fires neither self event"
        );
        // The resting bit's own edge, both ways.
        assert_eq!(
            flag_events(step(&mut app, 0x400 | PLAYER_FLAGS_RESTING)),
            vec!["PLAYER_UPDATE_RESTING".to_string()]
        );
        assert_eq!(
            flag_events(step(&mut app, 0x400)),
            vec!["PLAYER_UPDATE_RESTING".to_string()],
            "the fire is on the XOR-diff, so the clear edge fires it too"
        );

        // `PLAYTIME_CHANGED`, the two play-time regimes together.
        assert_eq!(
            flag_events(step(&mut app, 0x400 | PLAYER_FLAGS_PARTIAL_PLAY_TIME)),
            vec!["PLAYTIME_CHANGED".to_string()]
        );
        assert_eq!(
            flag_events(step(
                &mut app,
                0x400 | PLAYER_FLAGS_PARTIAL_PLAY_TIME | PLAYER_FLAGS_NO_PLAY_TIME
            )),
            vec!["PLAYTIME_CHANGED".to_string()],
            "the second regime bit is the same arm, not a second event"
        );

        // Both arms from one dword, in the handler's order.
        assert_eq!(
            flag_events(step(&mut app, 0x400 | PLAYER_FLAGS_RESTING)),
            vec![
                "PLAYER_UPDATE_RESTING".to_string(),
                "PLAYTIME_CHANGED".to_string()
            ]
        );
    }

    /// The player-only refusal rides in the entry, so a boar in interact range is not inspectable.
    #[test]
    fn a_creature_target_enters_the_reach_map_at_its_real_distance() {
        use benilla_protocol::messages::ObjectFields;
        use bevy::ecs::system::RunSystemOnce;

        /// `UNIT_FIELD_FLAGS` bit 1 (NON_ATTACKABLE), which `can_attack` refuses.
        const NON_ATTACKABLE: u32 = 1 << 1;
        const FIELD_UNIT_FLAGS: u16 = 46;
        const ME: u64 = 0x0000_0000_0000_0001;
        const BOAR: u64 = 0xF130_0000_0000_0002;
        const FRIEND: u64 = 0x0000_0000_0000_0003;

        /// Seat one target `yards` away and answer `expr` against it.
        fn ask(guid: u64, flags: u32, yards: f32, expr: &str) -> bool {
            let mut app = App::new();
            app.init_resource::<crate::ui_party::GroupState>()
                .init_resource::<Reputations>();
            app.insert_non_send_resource(UiScript::new().unwrap());

            let me = app
                .world_mut()
                .spawn((
                    SelfPlayer,
                    Guid(ME),
                    ObjectStore(ObjectFields::default()),
                    Transform::from_xyz(0.0, 0.0, 0.0),
                ))
                .id();
            let target = app
                .world_mut()
                .spawn((
                    Guid(guid),
                    ObjectStore(ObjectFields::from_pairs(&[(FIELD_UNIT_FLAGS, flags)])),
                    // Along one axis, so d² is exactly yards².
                    Transform::from_xyz(yards, 0.0, 0.0),
                ))
                .id();
            app.insert_resource(crate::net::GuidIndex(
                [(ME, me), (guid, target)].into_iter().collect(),
            ));
            app.insert_resource(Selection {
                target: Some(target),
                guid: Some(guid),
                ..Default::default()
            });
            // The resolver's inputs the aura feed would push: us and the target, both held.
            app.world_mut()
                .non_send_resource_mut::<UiScript>()
                .set_unit_guids(&benilla_ui::script::UnitGuids {
                    player: ME,
                    target: guid,
                    held: HashMap::from([(ME, 0), (guid, 0)]),
                    ..Default::default()
                });

            app.world_mut().run_system_once(feed_unit_reach).unwrap();
            app.world_mut()
                .non_send_resource_mut::<UiScript>()
                .eval::<bool>(expr)
                .unwrap()
        }

        assert!(
            ask(
                BOAR,
                0,
                40.0,
                r#"return CheckInteractDistance("target", 4) == nil"#
            ),
            "a creature 40 yards away is out of the 30-yard row"
        );
        assert!(
            ask(
                BOAR,
                0,
                15.0,
                r#"return CheckInteractDistance("target", 4) ~= nil"#
            ),
            "a creature 15 yards away is inside the 30-yard row"
        );
        assert!(
            ask(
                BOAR,
                0,
                15.0,
                r#"return CheckInteractDistance("target", 1) == nil"#
            ),
            "…and outside the 10-yard one: the table is indexed, not a constant"
        );

        assert!(
            ask(
                BOAR,
                0,
                3.0,
                r#"return CheckInteractDistance("target", 1) ~= nil"#
            ),
            "a creature 3 yards away is in interact range"
        );
        assert!(
            ask(BOAR, 0, 3.0, r#"return CanInspect("target") == nil"#),
            "…and is still not inspectable — the players-only leg rides in the entry"
        );
        // The control: a non-attackable player there is inspectable, so the refusal is the type.
        assert!(
            ask(
                FRIEND,
                NON_ATTACKABLE,
                3.0,
                r#"return CanInspect("target") ~= nil"#
            ),
            "a non-attackable player 3 yards away is inspectable"
        );
    }

    /// `UNIT_DYNFLAG_DEAD` alone makes empty bars over real maxima, through `snapshot`'s wiring.
    #[test]
    fn a_feigning_unit_snapshots_empty_bars_over_a_real_maximum() {
        use benilla_protocol::messages::ObjectFields;

        /// `UNIT_FIELD_HEALTH` / `MAXHEALTH` / `POWER1` / `MAXPOWER1` / `DYNAMIC_FLAGS`.
        const HEALTH: u16 = 22;
        const POWER1: u16 = 23;
        const MAXHEALTH: u16 = 28;
        const MAXPOWER1: u16 = 29;
        const DYNFLAGS: u16 = 143;

        let vitals = [
            (HEALTH, 1200),
            (MAXHEALTH, 1500),
            (POWER1, 300),
            (MAXPOWER1, 900),
        ];
        let alive = snapshot(
            &ObjectStore(ObjectFields::from_pairs(&vitals)),
            0,
            Some("Hunter".into()),
            0,
            None,
            Default::default(),
        );
        assert_eq!((alive.health, alive.max_health), (1200, 1500));
        assert_eq!((alive.power, alive.max_power), (300, 900));
        assert!(!alive.dead);

        let feigning = snapshot(
            &ObjectStore(ObjectFields::from_pairs(
                &[vitals.as_slice(), &[(DYNFLAGS, 0x20)]].concat(),
            )),
            0,
            Some("Hunter".into()),
            0,
            None,
            Default::default(),
        );
        assert_eq!(
            (feigning.health, feigning.max_health),
            (0, 1500),
            "UnitHealth zeroes, UnitHealthMax does not — an EMPTY bar, not a gone one"
        );
        assert_eq!(
            (feigning.power, feigning.max_power),
            (0, 900),
            "UnitMana zeroes, UnitManaMax does not"
        );
        assert!(feigning.dead, "UnitIsDead's dynflag leg");
        assert!(
            !feigning.ghost,
            "feign is not a ghost — PLAYER_FLAGS is clear"
        );

        // The flag moves only health and power, the pair the reference's watcher fires.
        assert_ne!(alive.health, feigning.health);
        assert_ne!(alive.power, feigning.power);
        assert_eq!(
            (alive.max_health, alive.max_power, alive.level),
            (feigning.max_health, feigning.max_power, feigning.level),
        );
    }

    /// The rank getter's two gates, through `enrich_unit`'s wiring.
    #[test]
    fn the_rank_gate_zeroes_a_pet_or_charm() {
        use benilla_protocol::messages::ObjectFields;

        const ENTRY: u32 = 12397; // Ol' Sooty, a rank-1 elite
        const GUID: u64 = (0xF130u64 << 48) | ((ENTRY as u64) << 24) | 0x42;
        /// `UNIT_FIELD_PETNUMBER`, absolute descriptor index (`OBJECT_END(6) + 0x85`).
        const PETNUMBER: u16 = 139;

        let mut names = NameCache::default();
        names.insert_creature(
            ENTRY,
            Some(crate::names::CreatureRecord {
                cursor_name: None,
                name: "Ol' Sooty".into(),
                subname: None,
                creature_type: 1,
                pet_family: 4, // Bear, a real tameable family
                rank: 1,
                type_flags: 0,
                civilian: false,
                racial_leader: false,
                display_id: 0,
                ..Default::default()
            }),
        );

        let rank_of = |fields: &[(u16, u32)]| {
            let store = ObjectStore(ObjectFields::from_pairs(fields));
            let mut s = UnitState::default();
            enrich_unit(&mut s, GUID, &names, &store, None, None);
            s.rank
        };

        assert_eq!(rank_of(&[]), 1, "a free elite keeps its template rank");
        assert_eq!(
            rank_of(&[(PETNUMBER, 0)]),
            1,
            "an explicit zero pet number is not a pet"
        );
        assert_eq!(
            rank_of(&[(PETNUMBER, 7)]),
            0,
            "a non-zero pet number forces rank 0 — no dragon on an enslaved elite"
        );

        // The record gate: rank 0 until the creature query answers.
        let store = ObjectStore(ObjectFields::from_pairs(&[]));
        let mut s = UnitState::default();
        enrich_unit(&mut s, GUID ^ (1 << 24), &names, &store, None, None);
        assert_eq!(s.rank, 0, "an un-queried creature has no classification");
    }

    /// `UnitFactionGroup("player")` takes the side from the race (`0x60d0f4`), so GM mode's
    /// template 35, which names none, keeps it; any other token reads the live template.
    #[test]
    fn the_player_faction_group_follows_the_race_under_a_gm_template() {
        use benilla_protocol::messages::ObjectFields;

        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let catalog = benilla_formats::load_faction_catalog(&mut chain).expect("Faction.dbc");
        let factions = crate::target::Factions::from_catalog(catalog);
        let side = |race: u32| {
            let gm = ObjectStore(ObjectFields::from_pairs(&[(36, race), (35, 35)]));
            assert_eq!(
                faction_group(&gm, Some(&factions)),
                None,
                "template 35 has no side"
            );
            race_group_mask(&gm, Some(&factions))
                .and_then(|(c, m)| c.faction_group_internal_name(m).map(str::to_string))
        };
        assert_eq!(side(3).as_deref(), Some("Alliance"), "a dwarf");
        assert_eq!(side(2).as_deref(), Some("Horde"), "an orc");
    }

    /// The entry gate passes with no record; the record's `HIDE_FACTION_TOOLTIP` (`0x10`) can then
    /// take the line away, as in the reference.
    #[test]
    fn the_faction_line_does_not_wait_for_the_creature_query() {
        use benilla_protocol::messages::ObjectFields;

        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let catalog = benilla_formats::load_faction_catalog(&mut chain).expect("Faction.dbc");
        let factions = crate::target::Factions::from_catalog(catalog);

        /// `UNIT_FIELD_FACTIONTEMPLATE` and `UNIT_FIELD_BYTES_0`, absolute descriptor indices.
        const FACTIONTEMPLATE: u16 = 35;
        const BYTES_0: u16 = 36;
        /// Race 1 and class 1 in `UNIT_FIELD_BYTES_0` bytes 0 and 1, for the slot walk.
        const HUMAN_WARRIOR: u32 = 1 | (1 << 8);
        /// A creature entry the cache is never told about.
        const ENTRY: u32 = 299;
        const GUID: u64 = (0xF130u64 << 48) | ((ENTRY as u64) << 24) | 0x7;

        let me = ObjectStore(ObjectFields::from_pairs(&[(BYTES_0, HUMAN_WARRIOR)]));
        // The first template whose faction shows this character a line, from the real DBC.
        let (template_id, expected) = (1u32..3000)
            .find_map(|id| {
                let f = factions.catalog().template(id)?.faction;
                let info = factions.catalog().reputation_faction(f)?;
                info.tooltip_shows_for(1, 1)
                    .then(|| factions.catalog().faction_name(f))
                    .flatten()
                    .map(|n| (id, n.to_string()))
            })
            .expect("some faction template shows a tooltip line to a human warrior");
        let store = ObjectStore(ObjectFields::from_pairs(&[(FACTIONTEMPLATE, template_id)]));

        let line_for = |names: &NameCache| {
            let mut state = UnitState::default();
            enrich_unit(&mut state, GUID, names, &store, Some(&factions), Some(&me));
            state
        };

        // The query in flight: no record, and the faction line all the same.
        let pending = line_for(&NameCache::default());
        assert_eq!(pending.name, None, "the name is the thing still in flight");
        assert_eq!(pending.subtitle, None);
        assert_eq!(
            pending.faction_name.as_deref(),
            Some(expected.as_str()),
            "the faction line resolves off the descriptor alone"
        );

        let record = |type_flags: u32| crate::names::CreatureRecord {
            cursor_name: None,
            name: "Stormwind Guard".into(),
            subname: None,
            creature_type: 7,
            pet_family: 0,
            rank: 0,
            type_flags,
            civilian: false,
            racial_leader: false,
            display_id: 0,
            ..Default::default()
        };
        let mut answered = NameCache::default();
        answered.insert_creature(ENTRY, Some(record(0)));
        assert_eq!(
            line_for(&answered).faction_name.as_deref(),
            Some(expected.as_str()),
            "the answer landing keeps the line it was already showing"
        );

        // The one creature-side gate the record does own.
        let mut hidden = NameCache::default();
        hidden.insert_creature(ENTRY, Some(record(0x10)));
        assert_eq!(
            line_for(&hidden).faction_name,
            None,
            "HIDE_FACTION_TOOLTIP takes the line away once the record says so"
        );
    }

    /// Asserted on keys: English text cannot tell apart two keys that agree in enUS.
    #[test]
    fn pvp_announcement_speaks_only_on_an_edge() {
        assert_eq!(
            pvp_announcement(None, false),
            None,
            "first sight, unflagged"
        );
        assert_eq!(pvp_announcement(None, true), None, "first sight, flagged");
        assert_eq!(pvp_announcement(Some(true), true), None, "no change");
        assert_eq!(pvp_announcement(Some(false), false), None, "no change");

        assert_eq!(
            pvp_announcement(Some(false), true),
            Some(("ERR_PVP_TOGGLE_ON", "PVP_TOGGLE_ON_VERBOSE"))
        );
        assert_eq!(
            pvp_announcement(Some(true), false),
            Some(("ERR_PVP_TOGGLE_OFF", "PVP_TOGGLE_OFF_VERBOSE"))
        );
    }

    /// A mistyped key silences a line; the OFF verbose sentence explains the five-minute wait.
    #[test]
    fn the_pvp_and_rest_keys_resolve_in_the_real_global_strings() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let src = chain
            .read_file("Interface\\FrameXML\\GlobalStrings.lua")
            .expect("GlobalStrings.lua in the chain");
        let s = UiScript::new().expect("VM");
        s.run(&String::from_utf8_lossy(&src)).expect("runs clean");
        let g = |key: &str| s.lua().globals().get::<String>(key).unwrap_or_default();

        for (was, now) in [(false, true), (true, false)] {
            let (toast, verbose) = pvp_announcement(Some(was), now).expect("an edge speaks");
            assert!(!g(toast).is_empty(), "{toast} missing");
            assert!(!g(verbose).is_empty(), "{verbose} missing");
        }
        assert!(
            g("PVP_TOGGLE_OFF_VERBOSE").contains("five minutes"),
            "the OFF sentence is what tells the player the flag lingers"
        );
        for state in [1u8, 2] {
            let key = rest_state_message(0, state).expect("states 1 and 2 speak");
            assert!(!g(key).is_empty(), "{key} missing");
        }
    }

    /// Only states 1 and 2 speak, and only on a real byte change.
    #[test]
    fn rest_state_message_speaks_only_on_a_real_transition() {
        assert_eq!(rest_state_message(2, 1), Some("ERR_EXHAUSTION_RESTED"));
        assert_eq!(rest_state_message(1, 2), Some("ERR_EXHAUSTION_NORMAL"));
        assert_eq!(
            rest_state_message(0, 1),
            Some("ERR_EXHAUSTION_RESTED"),
            "0→1 IS a transition"
        );
        assert_eq!(
            rest_state_message(1, 1),
            None,
            "same byte re-sent — the mirror diff eats it"
        );
        assert_eq!(rest_state_message(2, 2), None);
        assert_eq!(
            rest_state_message(1, 0),
            None,
            "state 0 is the 0x1d1 sentinel: no message"
        );
        assert_eq!(
            rest_state_message(2, 3),
            None,
            "beta tiers are gated off (cmp esi,3; jae)"
        );
        assert_eq!(rest_state_message(1, 5), None);
    }

    /// A real `Update` schedule: a fresh `run_system_once` sees every resource as changed and
    /// would hold the gate open.
    #[test]
    fn the_npc_token_follows_the_interaction_npc_and_clears_with_it() {
        use crate::ui_session::InteractNpc;
        use benilla_protocol::messages::ObjectFields;

        const FIELD_UNIT_LEVEL: u16 = 34;
        // Two `HIGHGUID_UNIT` guids: the high word decides the family.
        const BROG: u64 = 0xF130_0000_9700_0001;
        const DOBBINS: u64 = 0xF130_0001_D100_0002;

        let mut app = App::new();
        app.init_resource::<UnitFeedState>()
            .init_resource::<Selection>()
            .init_resource::<NameCache>()
            .init_resource::<Reputations>()
            .init_resource::<crate::ui_party::GroupState>()
            .init_resource::<crate::ui_chat::ChatLog>()
            // `show_messages`' sink is the chat log and the message-sound queue.
            .init_resource::<crate::sound::MessageSounds>()
            .init_resource::<crate::ui_guild::GuildState>()
            .init_resource::<InteractNpc>();
        app.add_message::<FieldChanged>();
        let (tx, _rx) = crossbeam_channel::unbounded();
        app.insert_resource(NetCommands(tx));
        app.insert_non_send_resource(UiScript::new().unwrap());
        app.add_systems(Update, feed_units);
        // Two vendors, told apart by level alone.
        let mut vendor = |level: u32| {
            app.world_mut()
                .spawn(ObjectStore(ObjectFields::from_pairs(&[(
                    FIELD_UNIT_LEVEL,
                    level,
                )])))
                .id()
        };
        let brog = vendor(7);
        let dobbins = vendor(9);
        let eval = |app: &mut App, expr: &str| -> i64 {
            app.world_mut()
                .non_send_resource_mut::<UiScript>()
                .eval::<i64>(expr)
                .unwrap()
        };
        let exists = |app: &mut App| eval(app, r#"return UnitExists("npc") and 1 or 0"#) == 1;
        let level = |app: &mut App| eval(app, r#"return UnitLevel("npc")"#);

        app.update();
        assert!(!exists(&mut app), "no window open, yet UnitExists(\"npc\")");

        *app.world_mut().resource_mut::<InteractNpc>() = InteractNpc(Some(brog), Some(BROG));
        app.update();
        assert_eq!(
            level(&mut app),
            7,
            "the token names the vendor whose window opened"
        );

        // Dobbins' window opens over it: the interaction NPC is the only thing that moved.
        *app.world_mut().resource_mut::<InteractNpc>() = InteractNpc(Some(dobbins), Some(DOBBINS));
        app.update();
        assert_eq!(
            level(&mut app),
            9,
            "a second vendor over an open window swaps the token"
        );

        *app.world_mut().resource_mut::<InteractNpc>() = InteractNpc::default();
        app.update();
        assert!(
            !exists(&mut app),
            "the window closed, yet UnitExists(\"npc\")"
        );
    }

    /// `UnitCreatureType` through the real feed: a shapeshifted player answers the form's type
    /// before the race's, an unshifted one the race's, a form of type -1 falls through to the race,
    /// and a creature keeps its template's unless a form outranks it.
    #[test]
    fn unit_creature_type_names_the_form_before_the_race_and_a_creature_its_template() {
        use crate::names::CreatureRecord;
        use benilla_formats::ShapeshiftForm;
        use benilla_protocol::messages::ObjectType;
        use benilla_protocol::ObjectFields;

        /// `UNIT_FIELD_BYTES_0` and `UNIT_FIELD_BYTES_1`, absolute descriptor indices, and
        /// `OBJECT_FIELD_ENTRY`.
        const BYTES_0: u16 = 36;
        const BYTES_2: u16 = 164;
        const OBJECT_FIELD_ENTRY: u16 = 3;
        const NIGHT_ELF: u32 = 4;
        const CAT_FORM: u32 = 1;
        /// A row whose type is -1 in the shipped table.
        const NO_TYPE_FORM: u32 = 2;
        /// `HIGHGUID_UNIT` guids of entries 69 (a wolf, a Beast) and 70 (a druid, a Humanoid).
        const WOLF: u64 = 0xF130_0000_4500_0001;
        const DRUID: u64 = 0xF130_0000_4600_0001;

        let record = |name: &str, creature_type| CreatureRecord {
            cursor_name: None,
            name: name.into(),
            subname: None,
            creature_type,
            pet_family: 0,
            rank: 0,
            type_flags: 0,
            civilian: false,
            racial_leader: false,
            display_id: 0,
            ..Default::default()
        };
        let mut app = App::new();
        app.init_resource::<UnitFeedState>()
            .init_resource::<Selection>()
            .init_resource::<Reputations>()
            .init_resource::<crate::ui_party::GroupState>()
            .init_resource::<crate::ui_chat::ChatLog>()
            .init_resource::<crate::sound::MessageSounds>()
            .init_resource::<crate::ui_guild::GuildState>();
        app.add_message::<FieldChanged>();
        let (tx, _rx) = crossbeam_channel::unbounded();
        app.insert_resource(NetCommands(tx));
        app.insert_non_send_resource(UiScript::new().unwrap());
        let mut names = NameCache::default();
        names.insert_creature(69, Some(record("Wolf", 1)));
        names.insert_creature(70, Some(record("Druid", 7)));
        app.insert_resource(names);
        let mut spells = crate::ui_action::Spells::empty_for_tests();
        let form = |creature_type| ShapeshiftForm {
            creature_type,
            ..Default::default()
        };
        spells.forms.insert(CAT_FORM, form(1));
        spells.forms.insert(NO_TYPE_FORM, form(-1));
        app.insert_resource(spells);
        app.add_systems(Update, feed_units);

        let me = app
            .world_mut()
            .spawn((
                SelfPlayer,
                Guid(0x77),
                ObjectStore(
                    ObjectFields::from_pairs(&[(BYTES_0, NIGHT_ELF)])
                        .into_created(ObjectType::Player),
                ),
            ))
            .id();
        let mut creature = |entry: u32| {
            app.world_mut()
                .spawn(ObjectStore(ObjectFields::from_pairs(&[(
                    OBJECT_FIELD_ENTRY,
                    entry,
                )])))
                .id()
        };
        let wolf = creature(69);
        let druid = creature(70);
        let creature_type = |app: &mut App, token: &str| -> Option<String> {
            app.world_mut()
                .non_send_resource_mut::<UiScript>()
                .eval::<Option<String>>(&format!(r#"return UnitCreatureType("{token}")"#))
                .unwrap()
        };
        let shift = |app: &mut App, unit: Entity, form: u32| {
            app.world_mut()
                .entity_mut(unit)
                .get_mut::<ObjectStore>()
                .unwrap()
                .0
                .merge(ObjectFields::from_pairs(&[(BYTES_2, form << 24)]));
            app.update();
        };
        let select = |app: &mut App, unit: Entity, guid: u64| {
            let mut selection = app.world_mut().resource_mut::<Selection>();
            selection.target = Some(unit);
            selection.guid = Some(guid);
            app.update();
        };

        app.update();
        assert_eq!(
            creature_type(&mut app, "player").as_deref(),
            Some("Humanoid"),
            "no form: the race's type"
        );
        shift(&mut app, me, CAT_FORM);
        assert_eq!(
            creature_type(&mut app, "player").as_deref(),
            Some("Beast"),
            "Cat Form: the form's type, before the race's"
        );
        shift(&mut app, me, NO_TYPE_FORM);
        assert_eq!(
            creature_type(&mut app, "player").as_deref(),
            Some("Humanoid"),
            "a form of type -1 falls through to the race"
        );
        shift(&mut app, me, 0);
        assert_eq!(
            creature_type(&mut app, "player").as_deref(),
            Some("Humanoid"),
            "out of form again"
        );

        select(&mut app, wolf, WOLF);
        assert_eq!(
            creature_type(&mut app, "target").as_deref(),
            Some("Beast"),
            "a creature's template type"
        );
        select(&mut app, druid, DRUID);
        assert_eq!(
            creature_type(&mut app, "target").as_deref(),
            Some("Humanoid"),
            "a Humanoid template"
        );
        shift(&mut app, druid, CAT_FORM);
        assert_eq!(
            creature_type(&mut app, "target").as_deref(),
            Some("Beast"),
            "a form outranks the template, on a creature as on a player"
        );
    }

    /// Event `0x111` fires from the local player's destructor, which a same-map teleport never
    /// reaches, and the initial-login map needs no ack.
    #[test]
    fn a_cross_map_worldport_fires_leaving_world_and_the_login_map_does_not() {
        let mut app = App::new();
        app.add_message::<crate::net::WorldportMessage>()
            .init_resource::<crate::ui_script::LeavingWorldArmed>()
            .add_message::<crate::ui_script::WorldLeaveSweepMessage>()
            .add_systems(Update, fire_leaving_world_on_worldport);
        app.insert_non_send_resource(UiScript::new().expect("VM"));
        app.world_mut()
            .non_send_resource::<UiScript>()
            .run(
                "Left = 0 \
                 local f = CreateFrame(\"Frame\") \
                 f:RegisterEvent(\"PLAYER_LEAVING_WORLD\") \
                 f:SetScript(\"OnEvent\", function() Left = Left + 1 end)",
            )
            .expect("probe frame");
        let left = |app: &mut App| -> i64 {
            app.world_mut()
                .non_send_resource_mut::<UiScript>()
                .eval::<i64>("return Left")
                .unwrap()
        };
        let port = |needs_ack: bool| crate::net::WorldportMessage {
            map_id: 1,
            position: [0.0; 3],
            orientation: 0.0,
            needs_ack,
            transport_entry: None,
        };

        let arm = |app: &mut App| {
            app.world_mut()
                .resource_mut::<crate::ui_script::LeavingWorldArmed>()
                .arm();
        };

        // A world began: the reference arms the latch from the local player's create.
        arm(&mut app);
        app.world_mut().write_message(port(false));
        app.update();
        assert_eq!(left(&mut app), 0, "the initial-login map is an arrival");

        app.world_mut().write_message(port(true));
        app.update();
        assert_eq!(left(&mut app), 1, "a cross-map port leaves a world");

        app.update();
        assert_eq!(
            left(&mut app),
            1,
            "once per port, not once per frame after it"
        );

        // Once per world: the port above spent the latch and no new avatar re-armed it, so a
        // second departure (a quit on the loading screen) fires nothing, as in the reference.
        app.world_mut().write_message(port(true));
        app.update();
        assert_eq!(
            left(&mut app),
            1,
            "a second departure with the latch spent fired again — the reference latch is per world"
        );

        // Re-armed, as the new world's create does: the next departure is its own.
        arm(&mut app);
        app.world_mut().write_message(port(true));
        app.update();
        assert_eq!(left(&mut app), 2, "the next world's departure fires again");
    }

    /// Every combo packet fires `UNIT_COMBO_POINTS` to the tokens naming its unit, an unchanged
    /// bank too, and the pet's adds `PET_BAR_UPDATE_USABLE`; a fresh VM takes the banks silently
    /// (`0x5d34b0`).
    #[test]
    fn each_combo_packet_fires_unit_combo_points_to_its_unit() {
        const ME: u64 = 0x7;
        const PET: u64 = 0xF140_0000_0000_0001;
        const MOB: u64 = 0xF130_0000_0000_0009;
        let mut world = World::new();
        world.insert_resource(crate::net::SelfGuid(Some(ME)));
        world.init_resource::<crate::spell::ComboPoints>();
        let mut bar = crate::ui_pet::PetBar::default();
        bar.spells.pet_guid = PET;
        world.insert_resource(bar);
        let mut s = UiScript::new().unwrap();
        s.set_unit_guids(&benilla_ui::script::UnitGuids {
            player: ME,
            pet: PET,
            ..Default::default()
        });
        s.run(
            r#"
            SEEN = {}
            local f = CreateFrame("Frame")
            f:RegisterEvent("UNIT_COMBO_POINTS")
            f:RegisterEvent("PET_BAR_UPDATE_USABLE")
            f:SetScript("OnEvent", function(self, event, unit) table.insert(SEEN, event .. ":" .. tostring(unit)) end)
        "#,
        )
        .unwrap();
        world.insert_non_send_resource(s);
        // One registered system, so its memo outlives each run as the app's does.
        let feed = world.register_system(feed_combo_points);
        let bank = |world: &mut World, pet: bool, points: u8| {
            let mut c = world.resource_mut::<crate::spell::ComboPoints>();
            let banked = crate::spell::combo::Banked {
                target: MOB,
                points,
            };
            if pet {
                c.pet = banked;
                c.pet_epoch += 1;
            } else {
                c.player = banked;
                c.player_epoch += 1;
            }
        };
        let seen = |world: &mut World| -> Vec<String> {
            world.run_system(feed).unwrap();
            let s = world.non_send_resource_mut::<UiScript>();
            let out = s.eval::<Vec<String>>("return SEEN").unwrap();
            s.run("SEEN = {}").unwrap();
            out
        };

        bank(&mut world, false, 2);
        assert!(
            seen(&mut world).is_empty(),
            "a fresh VM takes the bank silently"
        );
        let points = |world: &mut World, unit: &str| -> i64 {
            world
                .non_send_resource_mut::<UiScript>()
                .eval(&format!("return GetComboPoints('{unit}', 'target')"))
                .unwrap()
        };
        world
            .non_send_resource_mut::<UiScript>()
            .set_player_req_state(benilla_ui::script::PlayerReqState {
                class_id: 4,
                ..Default::default()
            });
        world.non_send_resource_mut::<UiScript>().set_unit(
            "target",
            Some(UnitState {
                exists: true,
                guid: MOB,
                ..Default::default()
            }),
        );
        assert_eq!(points(&mut world, "player"), 2);

        bank(&mut world, false, 2);
        assert_eq!(
            seen(&mut world),
            ["UNIT_COMBO_POINTS:player"],
            "unchanged still fires"
        );
        assert!(seen(&mut world).is_empty(), "no packet, no event");

        bank(&mut world, true, 1);
        assert_eq!(
            seen(&mut world),
            ["UNIT_COMBO_POINTS:pet", "PET_BAR_UPDATE_USABLE:nil"]
        );
        assert_eq!(points(&mut world, "pet"), 1);
    }
}
