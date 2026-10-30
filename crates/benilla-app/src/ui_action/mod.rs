//! The app side of the action bar: the action table, the spell catalogs and the plugin. [`feed`]
//! pushes each slot's identity into the VM and [`state`] its per-frame cooldown, usability, range
//! and checked state; [`drain`] sends `UseAction` as a cast, swing, item use or macro, and
//! `PickupAction`/`PlaceAction` as `CMSG_SET_ACTION_BUTTON`.

use std::collections::{BTreeSet, HashMap};

use bevy::prelude::*;

use benilla_formats::SpellCatalog;
use benilla_protocol::messages::ActionButton;

use crate::ui_script::UiInput;
use crate::ui_unit::UnitFeed;
use benilla_assets::{AssetSet, LockRecover, WorldAssets};

pub(crate) mod cast_fail;
mod drain;
mod equipment_sets;
// A `UseAction` press, applied in call order by `crate::script_calls`.
pub(crate) use drain::{attack_nearest_probe, use_action, ActionPress, AttackPress, UseOutcome};
#[cfg(test)]
mod drain_tests;
pub(crate) mod drop_item;
mod errors;
mod feed;
#[cfg(test)]
mod feed_tests;
mod net;
mod ranks;
mod state;
pub(crate) mod toggle;
mod weapon_icon;

/// Every feed that pushes cooldown triples runs `.before` this set: [`state::feed_action_state`]
/// fires the cooldown events synchronously, so a triple pushed later goes unread until the next
/// cooldown change. The pet list's flush, `ui_pet::bar::fire_pet_cooldown_events`, runs `.after`
/// it on the same rule.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct CooldownEvents;

/// The slot-identity feed ([`feed::feed_actions`]), which fires `ACTIONBAR_SLOT_CHANGED`. The
/// spellbook feed runs `.before` it: the registrar `0x542030` re-sorts the book and fires
/// `SPELLS_CHANGED` (`0x542cd4`) and `LEARNED_SPELL_IN_TAB` (`0x542d16`) before it places the
/// spell (`0x542de2`), so a slot's `GetActionInfo` book index resolves at its own event.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct SlotIdentityFeed;

/// The rank pass over the bar's spell slots, which a local slot writer follows.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct ActionRanks;

pub(crate) use errors::{
    attack_actor_blocked, attack_actor_refusal, keyed_line, keyed_line_s, reagent_totem_refusal,
    show_messages, ui_error_text, CastErrors, CastFail, Caster, FillArg, MessageSink, MountErrors,
    PetTameFailures, Shown, ToolCheck, UiError, UiErrorKeys, UiErrorTexts,
};
pub(crate) use weapon_icon::{melee_auto_attack_icon, ranged_weapon_icon};

/// `Attack`, the auto-attack pseudo-spell: it toggles melee rather than casting. The use path
/// keys on this id; the icon keys on `SPELL_EFFECT_ATTACK`, which only 6603 carries.
pub(crate) const SPELL_ATTACK: u32 = 6603;

/// A slot word's pet-action kind, `0x10000000 | index` (`0x5a78c0`): the temporary bar's slots
/// name the pet bar's actions by index, never stored on the server.
pub(crate) const ACTION_KIND_PET: u8 = 0x10;

/// The first temporary bar slot (wire 120, `0x5ab800`), twelve long.
pub(crate) const TEMP_BAR_FIRST_SLOT: u8 = 120;

/// The temporary bar's owners (`CGActionBar::m_tempPageActiveFlags`, `0x5a7c20`): bit `0x1` the
/// pet's bar (`0x5d4ad0`). Any set pages the bonus bar to 5 (`0x5a83c0`) and pins
/// `GetActionBarPage` to 1 (`0x5a7fd0`).
#[derive(Resource, Default, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TempActionBar {
    pub(crate) flags: u32,
}

/// The pet bar's temporary-bar flag.
pub(crate) const TEMP_BAR_PET: u32 = 0x1;

/// The action slots (wire 0..143) and the known spells. The bar is client-authoritative:
/// [`drain::drain_action_sets`] writes it directly, and vmangos stores a slot without a reply
/// (`MasterPlayer::addActionButton`).
#[derive(Resource, Default)]
pub(crate) struct PlayerActions {
    /// Wire slot (0-based) to its packed action; the Lua action id is slot + 1.
    pub buttons: HashMap<u8, ActionButton>,
    /// The spell book (`SMSG_INITIAL_SPELLS`), ordered on purpose: the reference's first-hit scans
    /// pick between equal spells, 6478 and 22810 "Opening", by array order, which is ascending id:
    /// the server sends the book from a `std::map`.
    pub spells: BTreeSet<u32>,
    /// The saved equipment sets (`SMSG_EQUIPMENT_SET_LIST`), which a set slot names by id.
    pub equipment_sets: Vec<benilla_protocol::messages::EquipmentSet>,
    /// A list has arrived this session, empty or not: a new VM starts loaded from it.
    pub equipment_sets_listed: bool,
    /// Set by every book or bar change, cleared once the feed re-resolves slot identity, which a
    /// landed item template ([`crate::items::Items::template_epoch`]) also triggers. Item counts
    /// refresh every frame regardless.
    pub dirty: bool,
}

impl PlayerActions {
    /// A local write that the server never hears of, as the temporary bar's (`0x5ab800`); returns
    /// whether the slot moved.
    pub(crate) fn set_local(&mut self, slot: u8, packed: u32) -> bool {
        let fresh = (packed != 0).then_some(ActionButton {
            slot,
            action: packed & 0x00FF_FFFF,
            kind: (packed >> 24) as u8,
        });
        if self.buttons.get(&slot) == fresh.as_ref() {
            return false;
        }
        match fresh {
            Some(b) => {
                self.buttons.insert(slot, b);
            }
            None => {
                self.buttons.remove(&slot);
            }
        }
        self.dirty = true;
        true
    }

    /// One button write, the tail every `PlaceAction` shares (`0x5aae80`, then
    /// `CMSG_SET_ACTION_BUTTON`): the store, `dirty` for the feed (which fires
    /// `ACTIONBAR_SLOT_CHANGED`), and the send. `packed == 0` clears; vmangos and AzerothCore
    /// store the slot without a reply.
    pub(crate) fn write_slot(&mut self, slot: u8, packed: u32, net: &crate::net::NetCommands) {
        if packed == 0 {
            self.buttons.remove(&slot);
        } else {
            self.buttons.insert(
                slot,
                ActionButton {
                    slot,
                    action: packed & 0x00FF_FFFF,
                    kind: (packed >> 24) as u8,
                },
            );
        }
        self.dirty = true;
        let _ = net.0.send(crate::net::ClientCommand::SetActionButton {
            button: slot,
            packed,
        });
    }
}

/// The world right-click's GameObject openers, queued one frame for the cast ladder: the click
/// system cannot also hold the ladder's resources, and only the ladder sends a cast, so a chest
/// right-clicked mid-cast takes the reference's silent same-spell bail.
#[derive(Resource, Default)]
pub(crate) struct GoOpenerCasts(pub(crate) Vec<GoOpener>);

/// A right-click's resolved opener ([`crate::target::click::resolve_go_action`]).
#[derive(Clone, Copy, Debug)]
pub(crate) enum GoOpener {
    /// A known `OPEN_LOCK` spell, cast with a GameObject target block.
    Spell { spell_id: u32, go_guid: u64 },
    /// A carried key: `CMSG_USE_ITEM` with `TARGET_FLAG_GAMEOBJECT`, the object's guid in
    /// [`crate::ui_items::ItemUse::on_object`].
    Key(crate::ui_items::ItemUse),
}

/// The spell DBC catalogs, absent without client data; the cast-visual router shares the catalog.
#[derive(Resource, Clone)]
pub(crate) struct Spells {
    pub(crate) catalog: SpellCatalog,
    /// Form id to its `SpellShapeshiftForm.dbc` row: `BonusActionBar` pages the bar
    /// (`GetBonusBarOffset`), `flags1` holds the stance and cancel bits.
    pub(crate) forms: std::collections::HashMap<u32, benilla_formats::ShapeshiftForm>,
    /// `SpellRange.dbc`, the range lookup's inputs; empty on a failed load.
    pub(crate) ranges: benilla_formats::SpellRangeCatalog,
    /// `SpellCastTimes.dbc`, read through `CastingTimeIndex` by the cast-time lookup.
    pub(crate) cast_times: benilla_formats::SpellCastTimeCatalog,
    /// `SpellDuration.dbc`: the `$d`/`$o` tokens' source, through the duration lookup.
    pub(crate) durations: benilla_formats::SpellDurationCatalog,
    /// `SpellRadius.dbc`: the `$a` token's yards.
    pub(crate) radii: benilla_formats::SpellRadiusCatalog,
}

impl Spells {
    /// The cast time in ms as the reference's cast-time lookup returns it for argument 1, the
    /// tooltip's: the level-scaled `CastingTimeIndex` row, then spell-mod op 10, with no clamp, so
    /// row 18's negative time survives. A missing row returns 0 before the modifier. The cast-speed
    /// multiply is not applied.
    pub(crate) fn cast_time_unclamped_ms(
        &self,
        def: &benilla_formats::SpellDisplay,
        caster_level: u32,
        mods: &crate::spell::SpellModifiers,
    ) -> i32 {
        self.cast_times
            .get(def.casting_time_index)
            .map_or(0, |row| {
                let resolved = row.resolved_ms(caster_level, def.base_level);
                mods.apply(def, crate::spell::OP_CAST_TIME, resolved)
            })
    }

    /// [`Self::cast_time_unclamped_ms`] for argument 0, the cast validator's: a result at or below
    /// zero reads 0.
    pub(crate) fn cast_time_ms(
        &self,
        def: &benilla_formats::SpellDisplay,
        caster_level: u32,
        mods: &crate::spell::SpellModifiers,
    ) -> u32 {
        self.cast_time_unclamped_ms(def, caster_level, mods).max(0) as u32
    }

    /// The learned spell the spell book's learn hook keeps for an `Effect[0]` (`0x5420a5`), each
    /// learn overwriting it, so the book's last in ascending id order.
    pub(crate) fn learned_with_effect(&self, learned: &BTreeSet<u32>, effect: u32) -> Option<u32> {
        learned.iter().rev().copied().find(|id| {
            self.catalog
                .get(*id)
                .is_some_and(|s| s.effects[0] == effect)
        })
    }
}

#[cfg(test)]
impl Spells {
    pub(crate) fn empty_for_tests() -> Self {
        Spells {
            catalog: SpellCatalog::from_displays(HashMap::new()),
            forms: HashMap::new(),
            ranges: benilla_formats::SpellRangeCatalog::default(),
            cast_times: Default::default(),
            durations: Default::default(),
            radii: Default::default(),
        }
    }
}

/// The 5875 spell data; `None` skips where the install is absent.
#[cfg(test)]
pub(crate) fn real_spells() -> Option<Spells> {
    let data = benilla_formats::wow_data_or_skip!(None);
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    Some(Spells {
        catalog: benilla_formats::load_spell_catalog(&mut chain).expect("Spell.dbc"),
        forms: benilla_formats::load_shapeshift_forms(&mut chain).expect("forms"),
        ranges: benilla_formats::load_spell_ranges(&mut chain).expect("ranges"),
        cast_times: benilla_formats::load_spell_cast_times(&mut chain).expect("cast times"),
        durations: benilla_formats::load_spell_durations(&mut chain).expect("durations"),
        radii: benilla_formats::load_spell_radii(&mut chain).expect("radii"),
    })
}

/// The reference's learned-ability latches: at learn time the spell book's learn hook (`0x542030`)
/// stores a spell's id by its `Effect[0]` (switch `0x54209e`), and unlearning clears it
/// (`0x53fb37`..`0x53fb77`). The skin cursor and click read the slots through the skinning lookup
/// (`0x53bce0`, [`Self::skinning_spell`]), so a corpse whose slot is empty gets neither.
#[derive(Resource, Default)]
pub(crate) struct LearnedAbilities {
    /// `[0xbe5d6c + 4 × EffectMiscValue[0]]` (`0x5420c7`): the known `SPELL_EFFECT_SKINNING` spell
    /// of each [`skinning_type`], leather (the Skinning ranks), herb (32605), rock (32606) and
    /// bolts (49383).
    pub(crate) skinning: [Option<u32>; 4],
    /// `[0xbe5d7c]` (`0x5420d6`): the known `SPELL_EFFECT_SKIN_PLAYER_CORPSE` spell, Remove
    /// Insignia, the slot a player or corpse reads.
    pub(crate) skin_player_corpse: Option<u32>,
    /// The known `SPELL_EFFECT_FEED_PET` spell, latched; gate 3 of [`drop_item::drop_item_on_unit`]
    /// and the spell it casts.
    pub(crate) feed_pet: Option<u32>,
}

/// What the skinning lookup (`0x53bce0`) tells apart by the object's type mask.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum SkinSubject {
    /// Mask `0x90`, a player or a corpse: the Remove Insignia slot.
    PlayerOrCorpse,
    /// Mask `0x08`, a creature, with its cached template's type flags.
    Creature { type_flags: u32 },
}

impl SkinSubject {
    /// The subject `store` is; a creature without a cached template reads no type flags.
    pub(crate) fn of(
        store: &crate::net::ObjectStore,
        names: Option<&crate::names::NameCache>,
    ) -> Self {
        use benilla_protocol::messages::ObjectType;
        match store.0.object_type() {
            Some(ObjectType::Player | ObjectType::Corpse) => Self::PlayerOrCorpse,
            _ => Self::Creature {
                type_flags: store
                    .0
                    .object_entry()
                    .and_then(|e| names?.creature_record(e))
                    .map_or(0, |r| r.type_flags),
            },
        }
    }
}

/// The unit's skinning type (`0x715e50`): the creature template's type flags `0x100` herb (1),
/// `0x200` rock (2), `0x8000` bolts (3), in that order, otherwise leather (0).
pub(crate) fn skinning_type(type_flags: u32) -> usize {
    if type_flags & 0x100 != 0 {
        1
    } else if type_flags & 0x200 != 0 {
        2
    } else if type_flags & 0x8000 != 0 {
        3
    } else {
        0
    }
}

/// The GlobalString of the tooltip's skinnable line by [`skinning_type`] (`0x620ee0`).
pub(crate) fn skinnable_text_key(type_flags: u32) -> &'static str {
    [
        "UNIT_SKINNABLE_LEATHER",
        "UNIT_SKINNABLE_HERB",
        "UNIT_SKINNABLE_ROCK",
        "UNIT_SKINNABLE_BOLTS",
    ][skinning_type(type_flags)]
}

impl LearnedAbilities {
    /// The skinning lookup (`0x53bce0`): the spell the skin cursor, click and tooltip read for
    /// `subject`, `None` when its slot is empty.
    pub(crate) fn skinning_spell(&self, subject: SkinSubject) -> Option<u32> {
        match subject {
            SkinSubject::PlayerOrCorpse => self.skin_player_corpse,
            SkinSubject::Creature { type_flags } => self.skinning[skinning_type(type_flags)],
        }
    }
}

/// The "Remove Insignia" effect, latched into `[0xbe5d7c]` (`0x5420d6`).
const SPELL_EFFECT_SKIN_PLAYER_CORPSE: u32 = 0x74;

/// The effect the reference latches at learn time; only Feed Pet 6991 has it.
const SPELL_EFFECT_FEED_PET: u32 = 0x65;

/// Re-derives [`LearnedAbilities`] whenever the book changes, standing in for learn-time writes.
fn track_learned_abilities(
    actions: Res<PlayerActions>,
    spells: Option<Res<Spells>>,
    mut learned: ResMut<LearnedAbilities>,
) {
    let Some(spells) = spells else { return };
    if !actions.is_changed() && !spells.is_changed() {
        return;
    }
    // The last match in id order: each learned rank overwrites the reference's latch, and a rank
    // chain ascends by id (Skinning 8613, 8617, 8618, 10768).
    let last_with = |effect: u32, misc: Option<i32>| {
        actions.spells.iter().copied().rfind(|&id| {
            spells.catalog.get(id).is_some_and(|d| {
                d.effects[0] == effect && misc.is_none_or(|m| d.effect_misc_value[0] == m)
            })
        })
    };
    let (skinning, skin_player_corpse, feed_pet) = (
        [0, 1, 2, 3].map(|kind| last_with(benilla_formats::SPELL_EFFECT_SKINNING, Some(kind))),
        last_with(SPELL_EFFECT_SKIN_PLAYER_CORPSE, None),
        last_with(SPELL_EFFECT_FEED_PET, None),
    );
    if (skinning, skin_player_corpse, feed_pet)
        != (
            learned.skinning,
            learned.skin_player_corpse,
            learned.feed_pet,
        )
    {
        *learned = LearnedAbilities {
            skinning,
            skin_player_corpse,
            feed_pet,
        };
    }
}

/// `SpellMechanic.dbc`: the name in `SPELL_FAILED_PREVENTED_BY_MECHANIC`'s `%s` (reason `0x8d`).
#[derive(Resource)]
pub(crate) struct SpellMechanics {
    pub(crate) catalog: benilla_formats::SpellMechanicCatalog,
}

fn load_spell_mechanics(mut commands: Commands, assets: Option<Res<benilla_assets::WorldAssets>>) {
    let Some(assets) = assets else { return };
    let loaded = {
        let mut chain = benilla_assets::LockRecover::lock_recover(&*assets.chain);
        benilla_formats::load_spell_mechanic_catalog(&mut chain)
    };
    match loaded {
        Ok(catalog) => {
            debug!("ui_action: {} spell-mechanic name(s)", catalog.len());
            commands.insert_resource(SpellMechanics { catalog });
        }
        Err(e) => warn!(
            "ui_action: SpellMechanic.dbc failed to load — the crowd-control refusal drops the \
             mechanic name: {e:#}"
        ),
    }
}

pub(crate) struct UiActionPlugin;

impl Plugin for UiActionPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        equipment_sets::register(app);
        app.init_resource::<PlayerActions>()
            .init_resource::<TempActionBar>()
            .init_resource::<LearnedAbilities>()
            .init_resource::<CastErrors>()
            .init_resource::<MountErrors>()
            .init_resource::<PetTameFailures>()
            .init_resource::<UiErrorKeys>()
            .init_resource::<UiErrorTexts>()
            .init_resource::<GoOpenerCasts>()
            .add_systems(Startup, load_spells.after(AssetSet::Open))
            .add_systems(Startup, load_spell_mechanics.after(AssetSet::Open))
            .add_systems(
                Update,
                (
                    // The feeds ride with the unit feed, before the VM ticks: the rank pass before
                    // the identity feed that consumes its `dirty`, the state feed after it, so a
                    // slot's rank fix and first state land the same frame. The drains follow the
                    // input pass, in any order: a gesture fills only one of their queues.
                    ranks::normalize_action_ranks
                        .in_set(UnitFeed)
                        .in_set(ActionRanks)
                        .before(feed::feed_actions),
                    feed::feed_actions.in_set(UnitFeed).in_set(SlotIdentityFeed),
                    state::feed_action_state
                        .in_set(UnitFeed)
                        .in_set(CooldownEvents)
                        .after(feed::feed_actions),
                    drain::drain_action_sets.after(UiInput),
                    equipment_sets::drain_equipment_sets.after(UiInput),
                    // The target chain queues the openers earlier in the frame.
                    drain::drain_go_openers.after(UiInput),
                    // The latches must be current before the target chain's cursor reads them.
                    track_learned_abilities
                        .in_set(UnitFeed)
                        .after(feed::feed_actions),
                ),
            );
    }
}

fn load_spells(mut commands: Commands, assets: Option<Res<WorldAssets>>) {
    let Some(assets) = assets else { return };
    let loaded = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_spell_catalog(&mut chain)
    };
    match loaded {
        Ok(catalog) => {
            let forms = {
                let mut chain = assets.chain.lock_recover();
                benilla_formats::load_shapeshift_forms(&mut chain).unwrap_or_else(|e| {
                    warn!("ui_action: SpellShapeshiftForm.dbc failed — stance paging off: {e:#}");
                    Default::default()
                })
            };
            let ranges = {
                let mut chain = assets.chain.lock_recover();
                benilla_formats::load_spell_ranges(&mut chain).unwrap_or_else(|e| {
                    warn!("ui_action: SpellRange.dbc failed — range indicator off: {e:#}");
                    benilla_formats::SpellRangeCatalog::default()
                })
            };
            let cast_times = {
                let mut chain = assets.chain.lock_recover();
                benilla_formats::load_spell_cast_times(&mut chain).unwrap_or_else(|e| {
                    warn!("ui_action: SpellCastTimes.dbc failed — cast-time cell off: {e:#}");
                    Default::default()
                })
            };
            let durations = {
                let mut chain = assets.chain.lock_recover();
                benilla_formats::load_spell_durations(&mut chain).unwrap_or_else(|e| {
                    warn!("ui_action: SpellDuration.dbc failed — $d/$o tokens off: {e:#}");
                    Default::default()
                })
            };
            let radii = {
                let mut chain = assets.chain.lock_recover();
                benilla_formats::load_spell_radii(&mut chain).unwrap_or_else(|e| {
                    warn!("ui_action: SpellRadius.dbc failed — $a token off: {e:#}");
                    Default::default()
                })
            };
            info!(
                "ui_action: {} spells in the display catalog, {} shapeshift forms, {} range rows, \
                 {} cast times, {} durations, {} radii",
                catalog.len(),
                forms.len(),
                ranges.len(),
                cast_times.len(),
                durations.len(),
                radii.len()
            );
            commands.insert_resource(Spells {
                catalog,
                forms,
                ranges,
                cast_times,
                durations,
                radii,
            });
        }
        Err(e) => warn!("ui_action: Spell.dbc failed to load — bar icons disabled: {e:#}"),
    }
}

#[cfg(test)]
mod learned_tests {
    use super::*;

    /// The install's SKINNING spells file by `EffectMiscValue[0]` (`0x5420c7`): Skinning ranks
    /// leather, 32605 herb, 32606 rock, 49383 bolts; Remove Insignia 22027 its own slot.
    #[test]
    fn learned_skinning_spells_file_by_their_misc_value() {
        let Some(spells) = real_spells() else { return };
        let mut app = App::new();
        app.insert_resource(spells);
        app.insert_resource(PlayerActions {
            spells: [8613, 8617, 22027, 32605, 32606, 49383].into(),
            ..Default::default()
        });
        app.init_resource::<LearnedAbilities>();
        app.add_systems(Update, track_learned_abilities);
        app.update();
        let learned = app.world().resource::<LearnedAbilities>();
        assert_eq!(
            learned.skinning,
            [Some(8617), Some(32605), Some(32606), Some(49383)]
        );
        assert_eq!(learned.skin_player_corpse, Some(22027));
        // A miner without Skinning has an empty leather slot.
        let miner = LearnedAbilities {
            skinning: [None, None, learned.skinning[2], None],
            ..Default::default()
        };
        assert_eq!(
            miner.skinning_spell(SkinSubject::Creature { type_flags: 0 }),
            None
        );
    }
}
