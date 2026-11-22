//! The VM's live view of the client and the events a packet handler fires. 12340's verbs read the live object when called (`0x60f396`) and a handler fires its events once its changes apply (`0x520fe0`). [`LiveView`] is the [`LiveState`] over the `World` every call into Lua is lent: every system that calls into Lua is exclusive and lends the `World` ([`with_live_world`], [`lend_world`]), and a verb builds the view over it at the call ([`with_world_view`]). [`UiEvents`] is the queue a handler fires into, emptied into the VM once that handler's commands apply.

use benilla_ui::script::{LiveState, ScriptValue, UiScript};
use bevy::ecs::system::{SystemParam, SystemState};
use bevy::prelude::*;

use crate::ui_bank::BankOpen;
use crate::ui_quest::QuestGiver;
use crate::ui_quest_log::rows::{QuestAsks, RebuildMarks};
use crate::ui_quest_log::QuestExpiries;

/// What the verbs read live, a system parameter so a VM system lends it with its own access. The
/// index is optional, so an app without the net plugin still lends a view that holds nobody.
#[derive(SystemParam)]
pub(crate) struct LiveView<'w, 's> {
    index: Option<Res<'w, crate::net::GuidIndex>>,
    stores: Query<'w, 's, &'static crate::net::ObjectStore>,
    control: Option<Res<'w, crate::net::ControlFlag>>,
    self_guid: Option<Res<'w, crate::net::SelfGuid>>,
    far_sight_clear: Option<Res<'w, crate::player::view_subject::FarSightClear>>,
    death: crate::death::DeathView<'w, 's>,
    summon: crate::ui_summon::SummonView<'w>,
    battlefield: crate::ui_battlefield::BattlefieldView<'w, 's>,
    score: crate::ui_battlefield_score::ScoreView<'w>,
    arena: Option<Res<'w, crate::ui_arena::ArenaOpponents>>,
    honor: crate::ui_honor::HonorView<'w>,
    arena_teams: crate::ui_arena_team::ArenaTeamView<'w>,
    positions: crate::ui_battlefield_positions::PositionsView<'w, 's>,
    inspect: crate::ui_inspect::InspectLive<'w>,
    quest: crate::ui_quest_watch::QuestView<'w, 's>,
    clock: Option<Res<'w, crate::net::ServerWallClock>>,
    world_state_ui: crate::world_state_ui::WorldStateUiLive<'w>,
    item_range: crate::ui_quest_watch::ItemRangeLive<'w, 's>,
    quest_tooltip: crate::ui_quest_log::tooltip::QuestTooltipLive<'w, 's>,
    quest_rows: crate::ui_quest_log::rows::QuestRowLive<'w, 's>,
    net: Option<Res<'w, crate::net::NetCommands>>,
    bank_prices: Option<Res<'w, crate::ui_bank::BankPrices>>,
    item_tables: Option<Res<'w, crate::ui_items::feed::ItemTablesLive>>,
    /// The enchant catalog a bound item's trade stamp tests for a binding enchant (`0x708b40`).
    enchants: Option<Res<'w, crate::items::Enchants>>,
    /// The item cache, a refund ask's template test (`0x62d580`).
    items: Option<Res<'w, crate::items::Items>>,
    /// The item records' callbacks, which a verb's ask joins in turn with every subsystem's.
    record_waits: Option<NonSendMut<'w, crate::items::RecordWaits>>,
    /// The main-thread state a verb writes inside its call.
    pub(crate) lent: MainState<'w>,
    companions: crate::ui_companion::CompanionLive<'w, 's>,
    pets: crate::ui_pet_stats::PetView<'w>,
    possess: crate::ui_pet::possess::PossessLive<'w>,
    pub(crate) stable: crate::ui_stable::StableView<'w, 's>,
    pub(crate) cooldowns: Option<ResMut<'w, crate::spell::Cooldowns>>,
    pub(crate) pet_bar: Option<ResMut<'w, crate::ui_pet::PetBar>>,
    pub(crate) selection: Option<ResMut<'w, crate::target::Selection>>,
    /// The unit-token resolver's tables (`0x60abf0`), the target global apart: the selection's.
    units: crate::ui_unit::TokenView<'w, 's>,
    pub(crate) place: Option<ResMut<'w, crate::area::PlayerPlace>>,
    /// The player's cast state, which every loan owns: the pending cast (`0x805f30`), the queued
    /// on-swing strike, the auto-repeating spell (`0x7fe130`) and the ground-targeting cursor.
    pub(crate) cast: CastSlots<'w>,
    /// The spell book and the equipment sets.
    pub(crate) actions: Option<ResMut<'w, crate::ui_action::PlayerActions>>,
    /// The bar's slots, which every loan owns: the bar verbs write them in the call.
    pub(crate) slots: Option<ResMut<'w, crate::ui_action::ActionSlots>>,
    /// The items a send locked (`item+0x314` bit 0), read into each slot's `locked`.
    pub(crate) pending: Option<ResMut<'w, crate::pending_item_ops::PendingItemOps>>,
    /// The open trade-skill book (`[0xc235a4]`), the current and selected tests' trade arm.
    pub(crate) trade: Option<ResMut<'w, crate::ui_tradeskill::TradeSkillOpen>>,
    /// The usable caches (`0x53cf10`), which every loan owns: a verb's site refreshes them.
    pub(crate) usable: Option<ResMut<'w, crate::ui_action::usable_cache::UsableCaches>>,
    /// The macros' bound spells, a macro slot's spell (`0x5a89f0`), which the rebind owns.
    pub(crate) bound: Option<ResMut<'w, crate::ui_macro::MacroBoundSpells>>,
    /// The usable walk's reads beside the player's own state (`0x80b5d0`).
    usable_world: crate::ui_action::usable_cache::UsableWorld<'w>,
    /// The bar's pages: the temporary bar's owners and the bonus offset.
    pages: Option<Res<'w, crate::ui_action::TempActionBar>>,
    /// The talent tables a talent link's rank reads.
    talents: Option<Res<'w, crate::ui_talent::Talents>>,
    /// The bind point and subclass names a spell tooltip reads.
    tooltip: crate::ui_tooltip::spell_view::TooltipTables<'w>,
    /// Our auto-attack lock (`[player+0xa20]`) and auras, the current-spell test's reads.
    engaged: Query<
        'w,
        's,
        (),
        (
            With<crate::net::SelfPlayer>,
            With<crate::creature_anim::Engaged>,
        ),
    >,
    auras: Query<'w, 's, &'static crate::unit_auras::UnitAuras>,
    /// The wire, for the sends a verb makes inside its call.
    commands: Option<Res<'w, crate::net::NetCommands>>,
    /// The usability walk's other inputs, for `GetPetActionSlotUsable` (`0x5d5400`).
    walk: Option<crate::ui_pet::PetSlotWalk<'w, 's>>,
    /// The bar's pet, our seat and the pet's auras, for the bar's usability and active reads.
    pet_unit: Option<crate::ui_pet::PetUnit<'w, 's>>,
    /// The spell records, the cooldown query's category and global-cooldown legs.
    spells: Option<Res<'w, crate::ui_action::Spells>>,
    /// `SummonProperties.dbc`, the dismiss's no-dismiss row (`0x5d484c`).
    summons: Option<Res<'w, crate::spell::SummonSlots>>,
    /// The active mover (`[0xca1238]`), the dismiss's controlled-vehicle test (`0x5d4744`).
    mover: Query<'w, 's, &'static crate::net::Guid, With<crate::net::ActiveMover>>,
    /// `ChrClasses.dbc`, the pet book's token and the requirement state's class flags.
    classes: Option<Res<'w, crate::chr_classes::ChrClassTable>>,
    /// The rune table, which only the handlers and the regen tick write.
    runes: Option<Res<'w, crate::ui_rune::Runes>>,
    /// The rune costs and cost modifiers the cooldown read's rune leg takes (`0x807c37`).
    rune_costs: Option<Res<'w, crate::ui_rune::RuneCosts>>,
    spell_mods: Option<Res<'w, crate::spell::SpellModifiers>>,
    /// The player's book, which the book update builds (`0x53ca70`), and the rank chains.
    book: Option<Res<'w, crate::ui_spellbook::SpellBook>>,
    skill_lines: Option<Res<'w, crate::ui_spellbook::SkillLines>>,
    /// The totem table and the tool search's inputs.
    totems: crate::ui_totem::TotemLive<'w, 's>,
    /// The targeting cursor's unit and range checks, `SpellCanTargetUnit`'s (`0x804190`).
    bind: Option<crate::spell::targeting::BindChecks<'w, 's>>,
    /// The book update's school masks (`0x53cd48`), which the not-interruptible test reads.
    schools: Option<Res<'w, crate::ui_cast::InterruptSchools>>,
    /// Each unit's cast state on the unit, the in-flight test's started spell.
    casting: Query<'w, 's, &'static crate::creature_anim::Casting>,
    /// The requirement state's proficiency masks, standings and faction table.
    proficiencies: Option<Res<'w, crate::net::Proficiencies>>,
    reputations: Option<Res<'w, crate::net::Reputations>>,
    factions: Option<Res<'w, crate::target::Factions>>,
    /// The currency tables, the item cache and the bag icons.
    currency: crate::ui_currency::CurrencyLive<'w>,
    /// The bags' objects, item cache and catalogs, read at the call.
    containers: Option<crate::ui_items::ContainerLive<'w, 's>>,
    /// The innkeeper and talent master latches and their reach.
    talk: crate::ui_binder::TalkView<'w, 's>,
    /// The instance boot deadline and the healer poll's inputs.
    dialogs: crate::ui_dialog_verbs::DialogView<'w, 's>,
    /// `ItemSet.dbc`, the name cache and the bonuses' token inputs, read at the call.
    item_sets: Option<crate::ui_items::ItemSetLive<'w, 's>>,
    /// The item cache and the catalogs a record's names resolve through, read at the call.
    item_records: Option<crate::ui_items::ItemRecordLive<'w, 's>>,
    /// The caches a guild bank slot and log row resolve through at the call.
    guild_bank: Option<crate::ui_guild_bank::BankCatalogs<'w>>,
    /// The tabard designer's vendor, save latch and our guild's record.
    pub(crate) tabard: crate::ui_tabard::TabardView<'w, 's>,
    /// The barber shop, our body's dials and the catalogs its verbs read.
    barber: crate::ui_barber::BarberLive<'w, 's>,
    /// The gossip globals, the NPC's record and our coinage.
    gossip: crate::ui_gossip::GossipView<'w, 's>,
    /// The item-text reader's globals and what its draw reads.
    item_text: crate::ui_item_text::ItemTextLive<'w, 's>,
    /// The merchant globals and what its rows, costs and sends read.
    merchant: Option<crate::ui_merchant::MerchantLive<'w, 's>>,
    /// The taxi map's globals and what its fares and activates read.
    taxi: Option<crate::ui_taxi::TaxiLive<'w, 's>>,
    /// The trainer window and what its gates, descriptions and buys read.
    trainer: Option<crate::ui_trainer::TrainerLive<'w, 's>>,
    /// The charter windows and what their walks and sends read.
    petition: Option<crate::ui_petition::PetitionLive<'w, 's>>,
    /// Every dressing-room pane's model and what a try-on reads.
    pub(crate) dressup: crate::ui_dressup::DressUpLive<'w>,
}

/// The player's cast state: the pending cast (`0x805f30`), the queued on-swing strike, the
/// auto-repeating spell (`0x7fe130`) and the ground-targeting cursor.
#[derive(SystemParam)]
pub(crate) struct CastSlots<'w> {
    pub(crate) pending: Option<ResMut<'w, crate::spell::PendingCast>>,
    pub(crate) queued_melee: Option<ResMut<'w, crate::spell::QueuedMeleeSpell>>,
    pub(crate) auto_repeat: Option<ResMut<'w, crate::spell::AutoRepeatActive>>,
    pub(crate) targeting: Option<ResMut<'w, crate::spell::SpellTargeting>>,
    /// Each unit's running cast and channel and its not-interruptible bit (`+0xa34 & 8`).
    pub(crate) unit_casts: Option<ResMut<'w, crate::ui_cast::UnitCasts>>,
}

/// The main-thread state a verb writes inside its call, as 12340's verbs write the game's globals.
#[derive(SystemParam)]
pub(crate) struct MainState<'w> {
    pub(crate) signals: Option<NonSendMut<'w, UniqueSignals>>,
    pub(crate) asks: Option<NonSendMut<'w, QuestAsks>>,
    pub(crate) marks: Option<NonSendMut<'w, RebuildMarks>>,
    pub(crate) expiries: Option<NonSendMut<'w, QuestExpiries>>,
    pub(crate) giver: Option<NonSendMut<'w, QuestGiver>>,
    pub(crate) bank: Option<NonSendMut<'w, BankOpen>>,
    pub(crate) refunds: Option<NonSendMut<'w, crate::ui_socket::RefundRecords>>,
    pub(crate) guild_bank: Option<NonSendMut<'w, crate::ui_guild_bank::GuildBankState>>,
    pub(crate) spirit: Option<NonSendMut<'w, crate::ui_dialog_verbs::AreaSpiritHealer>>,
}

mod actions;

impl LiveView<'_, '_> {
    /// A held unit's fields: the verbs' unit lookup (`0x60c1f0`) asks for type mask 8 (`0x60c21e`).
    fn unit(&self, guid: u64) -> Option<&benilla_protocol::ObjectFields> {
        let &entity = self.index.as_ref()?.0.get(&guid)?;
        let fields = &self.stores.get(entity).ok()?.0;
        fields.is_unit().then_some(fields)
    }

    /// A held player's entity, the object lookup with the player type mask (`0x54c3e7`).
    fn player_entity(&self, guid: u64) -> Option<Entity> {
        let &entity = self.index.as_ref()?.0.get(&guid)?;
        let fields = &self.stores.get(entity).ok()?.0;
        (fields.object_type() == Some(benilla_protocol::messages::ObjectType::Player))
            .then_some(entity)
    }

    /// A held player's `(race, class)`, the talent target's lookup with the player type mask (`0x5c9f8f`).
    fn player_class(&self, guid: u64) -> Option<(u8, u8)> {
        let fields = &self.stores.get(self.player_entity(guid)?).ok()?.0;
        Some((fields.unit_race()?, fields.unit_class()?))
    }

    /// Our player's entity and store.
    fn me(&self) -> Option<(Entity, &crate::net::ObjectStore)> {
        let entity = self.player_entity(self.self_guid.as_ref()?.0?)?;
        Some((entity, self.stores.get(entity).ok()?))
    }

    /// The lent state a bag read takes.
    fn container_lent(&self) -> crate::ui_items::ContainerLent<'_> {
        crate::ui_items::ContainerLent {
            cooldowns: self.cooldowns.as_deref(),
            pending: self.pending.as_deref(),
            petitions: self.petition.as_ref().map(|p| p.records()),
            banker: self
                .lent
                .bank
                .as_deref()
                .is_some_and(|b| b.banker.is_some()),
        }
    }

    /// A held player's PvP side (`0x54d074`).
    fn player_side(&self, guid: u64) -> Option<i8> {
        let entity = self.player_entity(guid)?;
        let race = self.stores.get(entity).ok()?.0.unit_race()?;
        Some(crate::ui_unit::race_pvp_team(race))
    }
}

/// The spell answers' reads at the call: our player and pet off the object stores, the cast state
/// off the lent slots.
impl crate::ui_spell_info::CallReads for LiveView<'_, '_> {
    fn spell_usable(&self, spell_id: u32, pet: bool) -> Option<(bool, bool)> {
        self.usable_reads(|r| r.spell_usable(spell_id, pet).pair())
    }

    fn caster(&self, pet: bool) -> Option<crate::ui_spell_info::Caster<'_>> {
        let entity = if pet {
            let guid = self.pet_bar.as_deref()?.spells.pet_guid;
            *self.index.as_ref()?.0.get(&guid).filter(|_| guid != 0)?
        } else {
            self.me()?.0
        };
        let store = self.stores.get(entity).ok()?;
        let (unit, pos) = self.item_range.facts().at(entity)?;
        Some(crate::ui_spell_info::Caster {
            guid: store.0.object_guid()?,
            pos,
            store,
            level: store.0.unit_level().unwrap_or(0),
            unit,
        })
    }

    fn mods(&self) -> Option<&crate::spell::SpellModifiers> {
        self.spell_mods.as_deref()
    }

    fn weapon_icon(&self, ranged: bool) -> Option<String> {
        let (containers, (_, me)) = (self.containers.as_ref()?, self.me()?);
        if ranged {
            containers.ranged_texture(me)
        } else {
            Some(containers.attack_texture(me, &self.spells.as_deref()?.forms))
        }
    }

    fn cast_now(&self) -> crate::ui_spell_info::CastNow<'_> {
        let mut now = crate::ui_spell_info::CastNow::none();
        let me = self.me();
        let store = me.map(|(_, s)| s);
        if let (Some(pending), Some(melee)) = (
            self.cast.pending.as_deref(),
            self.cast.queued_melee.as_deref(),
        ) {
            let casting = me.and_then(|(e, _)| self.casting.get(e).ok());
            now.pending = crate::ui_spell_info::pending_spells(pending, melee, casting);
            now.items = pending.records().iter().filter_map(|r| r.item).collect();
        }
        now.auto_repeat = self.cast.auto_repeat.as_deref().and_then(|a| a.0);
        now.attacking = !self.engaged.is_empty();
        now.trade_line = self.trade.as_deref().and_then(|t| t.line);
        now.race_class = crate::spell::race_class(store);
        now.critter = store.and_then(|s| s.0.unit_critter());
        now.auras = me.and_then(|(e, _)| self.auras.get(e).ok());
        now.guid = store.and_then(|s| s.0.object_guid()).unwrap_or(0);
        if let Some(actions) = self.actions.as_deref() {
            now.known = &actions.spells;
        }
        if let Some(summons) = self.summons.as_deref() {
            now.summons = &summons.0;
        }
        now
    }

    fn unit_facts(&self, guid: u64) -> Option<crate::spell::RangeFacts> {
        self.item_range.facts().facts(guid)
    }
}

impl LiveState for LiveView<'_, '_> {
    fn unit_flags(&self, guid: u64) -> Option<u32> {
        self.unit(guid).map(|f| f.unit_flags())
    }

    fn unit_health(&self, guid: u64) -> Option<u32> {
        self.unit(guid).map(|f| f.unit_health().unwrap_or(0))
    }

    fn unit_dynamic_flags(&self, guid: u64) -> Option<u32> {
        self.unit(guid).map(|f| f.unit_dynamic_flags())
    }

    /// The type mask's player bit first, as `UnitIsGhost` tests it (`0x60f5fe`).
    fn player_flags(&self, guid: u64) -> Option<u32> {
        self.unit(guid)
            .filter(|f| f.object_type() == Some(benilla_protocol::messages::ObjectType::Player))
            .map(|f| f.player_flags())
    }

    fn active_player(&self) -> Option<u64> {
        self.self_guid.as_ref()?.0
    }

    /// An app without the net plugin boots in control.
    fn has_control(&self) -> bool {
        self.control.as_ref().is_none_or(|c| c.0)
    }

    /// Our `PLAYER_FARSIGHT` naming a held object, as the bit needs (`0x6e28c6`-`0x6e28ff`), and
    /// no immediate clear ending it (`0x6e2b00`).
    fn far_sight(&self) -> bool {
        let Some(index) = self.index.as_ref() else {
            return false;
        };
        let field = self
            .active_player()
            .and_then(|me| index.0.get(&me))
            .and_then(|&e| self.stores.get(e).ok())
            .and_then(|store| store.0.player_farsight());
        field.is_some_and(|subject| {
            index.0.contains_key(&subject)
                && self
                    .far_sight_clear
                    .as_ref()
                    .is_none_or(|c| !c.cuts(subject))
        })
    }

    fn release_time_left_ms(&self) -> Option<u32> {
        self.death.release_time_left_ms()
    }

    fn corpse_recovery_delay_ms(&self) -> u32 {
        self.death.corpse_recovery_delay_ms()
    }

    fn resurrect_sickness(&self) -> bool {
        self.death.resurrect_sickness()
    }

    fn resurrect_has_timer(&self) -> bool {
        self.death.resurrect_has_timer()
    }

    fn spirit_healer_in_range(&self) -> bool {
        self.death.spirit_healer_in_range()
    }

    fn res_sickness_minutes(&self) -> Option<u32> {
        self.death.res_sickness_minutes()
    }

    fn resurrect_offerer(&self) -> Option<String> {
        self.death.resurrect_offerer()
    }

    fn self_res_label(&self) -> Option<String> {
        self.death.self_res_label()
    }

    fn falling(&self) -> bool {
        self.death.falling()
    }

    fn out_of_bounds(&self) -> bool {
        self.death.out_of_bounds()
    }

    fn cannot_be_resurrected(&self) -> bool {
        self.death.cannot_be_resurrected()
    }

    fn summon_summoner(&self) -> String {
        self.summon.summoner()
    }

    fn summon_area(&self) -> String {
        self.summon.area()
    }

    fn summon_time_left_ms(&self) -> u32 {
        self.summon.time_left_ms()
    }

    fn item_spell_in_range(&self, spell_id: u32, target: u64) -> Option<Option<bool>> {
        Some(self.item_range.in_range(spell_id, target))
    }

    fn unique_signal(&mut self, event: &'static str) -> bool {
        self.lent
            .signals
            .as_deref_mut()
            .map(|s| s.unique_signal(event))
            .is_some()
    }

    fn world_state_ui_count(&self) -> Option<usize> {
        self.world_state_ui.count()
    }

    fn world_state_ui_info(
        &self,
        index: usize,
    ) -> Option<Option<benilla_ui::script::WorldStateUiView>> {
        self.world_state_ui.info(index)
    }

    fn world_state(&self, key: u32) -> Option<i32> {
        self.world_state_ui.value(key)
    }

    fn quest_tooltip(
        &mut self,
        quest_id: u32,
        get: &dyn Fn(&str) -> Option<String>,
    ) -> Option<benilla_ui::script::QuestTooltipRead> {
        self.quest_tooltip.read(quest_id, get)
    }

    fn quest_poi_lines(
        &self,
        quest_id: u32,
        get: &dyn Fn(&str) -> Option<String>,
    ) -> Option<Option<benilla_ui::script::QuestPoiLines>> {
        Some(self.quest.quest_poi_lines(quest_id, get))
    }

    fn quest_special_item(
        &self,
        quest_id: u32,
    ) -> Option<benilla_ui::script::QuestSpecialItemView> {
        self.quest.quest_special_item(quest_id)
    }

    fn quest_log_row(
        &mut self,
        quest_id: u32,
        slot: u8,
        get: &dyn Fn(&str) -> Option<String>,
    ) -> Option<Option<benilla_ui::script::QuestLogEntryView>> {
        let Self {
            quest_rows,
            lent,
            record_waits,
            ..
        } = self;
        let MainState { asks, expiries, .. } = lent;
        let lists = (asks.as_deref_mut(), record_waits.as_deref_mut());
        quest_rows.row(quest_id, slot, get, expiries.as_deref(), lists)
    }

    fn quest_log_detail(
        &mut self,
        quest_id: u32,
        slot: u8,
    ) -> Option<Option<benilla_ui::script::QuestLogDetail>> {
        let Self {
            quest_rows,
            lent,
            record_waits,
            ..
        } = self;
        let lists = (lent.asks.as_deref_mut(), record_waits.as_deref_mut());
        quest_rows.detail(quest_id, slot, lists)
    }

    fn daily_quests_completed(&self) -> Option<u32> {
        self.quest_rows.daily_completed()
    }

    fn expire_quest(&mut self, quest_id: u32) -> Option<Option<String>> {
        let Self { quest, lent, .. } = self;
        quest.expire_quest(quest_id, lent.expiries.as_deref_mut()?)
    }

    /// The last time response advanced on our monotonic clock, where the reference adds the local clock to its offset (`0x5dff40`). Deviation: a local clock step mid-session (an NTP correction) moves the reference's countdowns and not ours, as the step changes no time the server counts.
    fn server_unix_time(&self) -> Option<f64> {
        self.clock.as_ref()?.now_unix()
    }

    fn quest_reset_unix(&self) -> Option<f64> {
        self.clock.as_ref()?.quest_reset_unix()
    }

    fn send_quest_log_tail(&mut self, pois: &[u32], query_time: bool) {
        let Some(net) = self.net.as_deref() else {
            return;
        };
        if !pois.is_empty() {
            debug!("ui_quest_log: asking POI for {pois:?}");
            let _ = net.0.send(crate::net::ClientCommand::QuestPoiQuery {
                quests: pois.to_vec(),
            });
        }
        if query_time {
            let _ = net.0.send(crate::net::ClientCommand::QueryTime);
        }
    }

    fn rebuild_quest_log_if_reask_due(&mut self) -> Option<benilla_ui::script::QuestLogPlan> {
        let Self {
            quest_rows, lent, ..
        } = self;
        quest_rows.rebuild_if_reask_due(lent.row_writes()?)
    }

    fn quest_item_record(
        &self,
        item_id: u32,
    ) -> Option<Option<benilla_ui::script::QuestItemRecord>> {
        self.quest.quest_item_record(item_id)
    }

    fn quest_action(
        &mut self,
        action: benilla_ui::script::QuestAction,
    ) -> Option<benilla_ui::script::QuestActOutcome> {
        let Self {
            index,
            stores,
            net,
            lent,
            ..
        } = self;
        let giver = lent.giver.as_deref_mut()?;
        let held = giver.npc;
        let npc_flags = held
            .and_then(|npc| index.as_ref()?.0.get(&npc).copied())
            .and_then(|e| stores.get(e).ok())
            .map(|s| s.0.unit_npc_flags());
        let out = crate::ui_quest::act(giver, action, net.as_deref()?, npc_flags);
        // A finish with no gossip menu held clears the interact target on the old giver (`0x58cb83`-`0x58cb8a`, `0x58cdb7`-`0x58cde4`).
        if let (true, Some(npc), None) = (out.finished, held, self.gossip.npc()) {
            if let Some(t) = self.tabard.target_mut() {
                t.clear_in_call(npc);
            }
        }
        Some(out)
    }

    fn quest_npc(&self) -> Option<u64> {
        self.lent.giver.as_deref()?.quest_npc()
    }

    fn interact_target(&self) -> Option<u64> {
        self.tabard.target()
    }

    fn quest_select(
        &mut self,
        select: benilla_ui::script::QuestSelect,
    ) -> Option<benilla_ui::script::QuestActOutcome> {
        let Self { net, lent, .. } = self;
        Some(crate::ui_quest::select(
            lent.giver.as_deref_mut()?,
            select,
            net.as_deref()?,
        ))
    }

    fn carried_quest_item(&self, item_id: u32) -> Option<u32> {
        self.quest_rows.carried(item_id)
    }

    fn battlefield_slot(&self, index: usize) -> Option<benilla_ui::script::BattlefieldQueueSlot> {
        self.battlefield.slot(index)
    }

    fn battlefield_instance(&self) -> benilla_ui::script::BattlefieldInstance {
        self.battlefield.instance()
    }

    fn world_pvp_queue(&self) -> benilla_ui::script::WorldPvpQueueView {
        self.battlefield.world_pvp()
    }

    fn can_hearth_and_resurrect(&self) -> bool {
        self.battlefield.can_hearth_and_resurrect()
    }

    fn battlefield_port_refusal(&self) -> Option<&'static str> {
        self.battlefield.port_refusal()
    }

    fn battlemaster_row(&self, id: u32) -> Option<benilla_ui::script::BattlemasterRow> {
        self.battlefield.row(id)
    }

    fn battleground_types(&self) -> Vec<benilla_ui::script::BattlegroundType> {
        self.battlefield.types()
    }

    /// A held unit's own race and class stand in where the name answer carried none.
    fn score_name(&self, guid: u64) -> Option<benilla_ui::script::ScoreName> {
        self.score.name(guid, |g| {
            let f = self.unit(g)?;
            Some((f.unit_race()?, f.unit_class()?))
        })
    }

    fn battlefield_score_names_pending(&self) -> bool {
        self.score.names_pending()
    }

    fn arena_opponent_count(&self) -> usize {
        self.arena.as_ref().map_or(0, |a| a.count())
    }

    fn honor(&self) -> Option<benilla_ui::script::HonorState> {
        crate::ui_honor::honor_of(self.unit(self.active_player()?)?)
    }

    fn pvp_toggle_timer(&self) -> Option<i32> {
        let me = self.unit(self.active_player()?)?;
        Some(self.honor.toggle_timer(me))
    }

    fn arena_team_info(&self, slot: usize) -> [u32; 7] {
        let me = self.active_player().and_then(|g| self.unit(g));
        crate::ui_arena_team::ArenaTeamView::info(me, slot)
    }

    fn arena_team_stats(&self, slot: usize) -> [u32; 6] {
        let me = self.active_player().and_then(|g| self.unit(g));
        self.arena_teams.stats(me, slot)
    }

    fn arena_team_roster_type(&self, slot: usize) -> u32 {
        let me = self.active_player().and_then(|g| self.unit(g));
        self.arena_teams.roster_type(me, slot)
    }

    fn arena_team_member_count(&self, slot: usize) -> usize {
        let me = self.active_player().and_then(|g| self.unit(g));
        self.arena_teams.member_count(me, slot)
    }

    fn arena_team_member(
        &self,
        slot: usize,
        index: usize,
    ) -> Option<benilla_ui::script::ArenaTeamMemberRef<'_>> {
        let me = self.active_player().and_then(|g| self.unit(g));
        self.arena_teams.member(me, slot, index)
    }

    fn arena_team_record(&self, team_id: u32) -> Option<benilla_ui::script::ArenaTeamRecordView> {
        self.arena_teams.record(team_id)
    }

    fn battlefield_position_count(&self) -> usize {
        self.positions.count(self.active_player())
    }

    fn battlefield_position(
        &self,
        index: usize,
    ) -> Option<benilla_ui::script::BattlefieldPositionLive> {
        self.positions
            .position(self.active_player(), index, |g| self.player_entity(g))
    }

    fn battlefield_flag_count(&self) -> usize {
        self.positions.flag_count()
    }

    fn battlefield_flag(&self, index: usize) -> Option<benilla_ui::script::BattlefieldFlagLive> {
        self.positions.flag(
            self.active_player(),
            index,
            |g| self.player_entity(g),
            |g| self.player_side(g),
        )
    }

    fn battlefield_icon_scale(&self) -> f32 {
        self.positions.icon_scale()
    }

    fn request_battlefield_positions(&self) {
        self.positions.request();
    }

    fn notify_inspect(&self, guid: u64) -> bool {
        self.inspect.notify(guid, || self.player_class(guid))
    }

    fn clear_inspect_target(&self) {
        self.inspect.clear();
    }

    fn companion_spells(&self, ty: benilla_ui::script::CompanionType) -> Vec<u32> {
        self.companions.spells(ty)
    }

    fn companion(
        &self,
        ty: benilla_ui::script::CompanionType,
        index: usize,
    ) -> Option<benilla_ui::script::CompanionView> {
        self.companions.info(ty, index)
    }

    fn companion_random_pool(&self) -> Vec<u32> {
        self.companions.random_pool()
    }

    /// The pet list's head held as a unit (`0x5d3a4d`, type mask 8).
    fn pet_stats(&self) -> (bool, benilla_ui::script::PetStats) {
        let pet = self.pets.head().and_then(|g| self.unit(g));
        self.pets.stats(pet, |g| self.unit(g))
    }

    fn pet_menu(&self) -> (bool, bool, bool) {
        let pet = self.pets.head().and_then(|g| self.unit(g));
        self.pets.menu(pet, self.active_player())
    }

    fn creature_family(&self, guid: u64) -> Option<String> {
        self.pets.creature_family(self.unit(guid)?)
    }

    fn possess(&self) -> Option<benilla_ui::script::PossessView> {
        self.possess.view()
    }

    fn stable(&self) -> benilla_ui::script::StableList {
        self.stable.list()
    }

    fn stable_family(
        &mut self,
        entry: u32,
        callback: Option<benilla_ui::script::StableCallback>,
    ) -> Option<benilla_ui::script::StableFamily> {
        let me = self.active_player().unwrap_or(0);
        self.stable.family(entry, callback, me)
    }

    /// The pet list's head held as a unit with a pet number and a hunter creator held as a player (`0x71b630`), and its creature record cached (`0x5a1602`).
    fn stable_live_pet(&self) -> bool {
        let Some(pet) = self.pets.head().and_then(|g| self.unit(g)) else {
            return false;
        };
        let hunter = pet
            .unit_created_by()
            .and_then(|g| self.player_class(g))
            .is_some_and(|(_, class)| class == 3);
        pet.unit_pet_number() != 0
            && hunter
            && pet
                .object_entry()
                .is_some_and(|entry| self.stable.has_record(entry))
    }

    fn stable_slot_price(&self, slot: u32) -> Option<u32> {
        self.stable.slot_price(slot)
    }

    fn stable_send(&self, verb: benilla_ui::script::StableIntent) {
        self.stable.send(verb);
    }

    fn close_stables(&mut self) -> bool {
        // The close clears the interact target on the master (`0x5a0f2c`).
        let master = self
            .stable
            .open()
            .and_then(crate::ui_session::NpcSession::npc);
        if let (Some(master), Some(t)) = (master, self.tabard.target_mut()) {
            t.clear_in_call(master);
        }
        self.stable.close()
    }

    /// The pet list's head (`0x5d3a24`).
    fn pet_out(&self) -> bool {
        self.pets.head().is_some()
    }

    /// Our `UNIT_FIELD_CHARM` or `UNIT_FIELD_SUMMON` (`0x5a1b11`); no player object refuses too (`0x5a1b0b`).
    fn unit_pet(&self, guid: u64) -> Option<u64> {
        self.unit(guid)?.unit_pet_guid()
    }

    fn unit_base(&self, base: benilla_ui::script::UnitBase) -> Option<u64> {
        let units = self
            .units
            .at(self.selection.as_deref().and_then(|s| s.guid));
        Some(crate::ui_unit::tokens::base(&units, base).unwrap_or(0))
    }

    fn unit_token(
        &self,
        base: benilla_ui::script::UnitBase,
        hops: benilla_ui::script::Hops<'_>,
        focus: u64,
    ) -> Option<Option<u64>> {
        use benilla_ui::script::UnitBase;
        let units = self
            .units
            .at(self.selection.as_deref().and_then(|s| s.guid));
        let guid = match base {
            UnitBase::Focus => focus,
            // The interact target (`0x60af67`) and the quest window's source (`0x6081b0`).
            UnitBase::Npc => self.interact_target().unwrap_or(0),
            UnitBase::QuestNpc => self.quest_npc().unwrap_or(0),
            base => crate::ui_unit::tokens::base(&units, base).unwrap_or(0),
        };
        // A base naming nobody with text after it goes to the name lookup.
        Some(
            (guid != 0 || hops.is_empty())
                .then(|| crate::ui_unit::tokens::follow(&units, guid, hops))
                .flatten(),
        )
    }

    fn unit_hops(&self, guid: u64, hops: benilla_ui::script::Hops<'_>) -> Option<Option<u64>> {
        let units = self
            .units
            .at(self.selection.as_deref().and_then(|s| s.guid));
        Some(crate::ui_unit::tokens::follow(&units, guid, hops))
    }

    fn unit_tables(&self) -> Option<benilla_ui::script::UnitGuids> {
        let units = self
            .units
            .at(self.selection.as_deref().and_then(|s| s.guid));
        let mut tables = crate::ui_unit::tokens::tables(&units);
        tables.npc = self.interact_target().unwrap_or(0);
        Some(tables)
    }

    fn charm_or_summon(&self) -> bool {
        let Some(me) = self.active_player().and_then(|g| self.unit(g)) else {
            return true;
        };
        me.unit_charm().is_some() || me.unit_summon().is_some()
    }

    fn open_stable_aura(&self) -> bool {
        self.stable.open_stable_aura()
    }

    /// The pet setter's words and globals (`0x5d6900`) with the usability predicate (`0x5d3e80`).
    fn pet_bar(&self) -> Option<benilla_ui::script::PetBarLive> {
        let bar = self.pet_bar.as_deref()?;
        Some(crate::ui_pet::bar_live(bar, self.pet_unit.as_ref()))
    }

    /// The slot word at the call (`0x5d4ec0`), its cooldown asked of the pet's history (`0x5d5345`) and its usability walked at the call (`0x5d5400`).
    fn pet_action(
        &self,
        index: usize,
        anchor: std::time::Instant,
        ui_now: f64,
    ) -> Option<benilla_ui::script::PetActionView> {
        let lent = crate::ui_pet::WalkLent {
            selection: self.selection.as_deref(),
            place: self.place.as_deref(),
        };
        crate::ui_pet::action_live(
            self.pet_bar.as_deref()?,
            index,
            self.spells.as_deref(),
            self.pet_unit.as_ref(),
            self.walk.as_ref().map(|w| (w, lent)),
            (anchor, ui_now),
        )
    }

    /// The drag's write into the pet's words (`0x5d5c3e`), then its one packet (`0x5d5cda`).
    fn pet_place_action(&mut self, entries: &[(u32, u32)]) -> bool {
        let Some(bar) = self.pet_bar.as_deref_mut() else {
            return false;
        };
        for &(position, packed) in entries {
            if let Some(e) = bar.spells.bar.get_mut(position as usize) {
                *e = benilla_protocol::messages::PetActionEntry::from(packed);
            }
        }
        if let Some(c) = &self.commands {
            let _ = c.0.send(crate::net::ClientCommand::PetSetAction {
                pet_guid: bar.spells.pet_guid,
                entries: entries.to_vec(),
            });
        }
        true
    }

    /// A pet menu verb on the lent bar inside its call ([`crate::ui_pet::run_menu_verb`]).
    fn pet_menu_verb(
        &mut self,
        verb: benilla_ui::script::PetMenuVerb<'_>,
    ) -> Option<benilla_ui::script::PetMenuOutcome> {
        let stable_master = self
            .stable
            .open()
            .and_then(crate::ui_session::NpcSession::npc);
        let cx = crate::ui_pet::MenuCx {
            bar: self.pet_bar.as_deref()?,
            pet: self.pet_unit.as_ref()?,
            spells: self.spells.as_deref(),
            summons: self.summons.as_deref(),
            commands: self.commands.as_deref()?,
            stable_master,
            mover: self.mover.iter().next().map(|g| g.0),
        };
        Some(crate::ui_pet::run_menu_verb(&cx, verb))
    }

    /// `PetStopAttack`'s clear on the lent bar (`0x5d4650`).
    fn pet_stop_attack(&mut self) {
        if let (Some(bar), Some(c)) = (self.pet_bar.as_deref_mut(), &self.commands) {
            crate::ui_pet::stop_pet_attack(bar, c);
        }
    }

    /// A reaction or non-Attack command press on the lent words inside the call (`0x5d4210`): gate and latch; the VM signals, then sends.
    fn pet_press(&mut self, slot: u32, order: Option<u32>) -> Option<benilla_ui::script::PetLatch> {
        let entry = crate::ui_pet::plain_press_word(self.pet_bar.as_deref()?, slot, order)?;
        let selected = self.selection.as_deref().and_then(|s| s.guid);
        let pet = self.pet_unit.as_ref()?;
        let bar = self.pet_bar.as_deref_mut()?;
        Some(crate::ui_pet::press_plain(bar, pet, entry, selected))
    }

    /// The latched press's send, under the pet the bar holds now (`0x5d4585`).
    fn pet_press_send(&mut self, word: u32, target: u64) {
        if let (Some(bar), Some(c)) = (self.pet_bar.as_deref(), self.commands.as_deref()) {
            crate::ui_pet::send_pet_action(bar, c, word, target);
        }
    }

    /// The bar toggle (`0x5d5d20`) on the lent words: gate, flip, send, in 12340's order.
    fn pet_toggle_autocast(&mut self, index: u32) -> bool {
        let usable = self.pet_bar.as_deref().is_some_and(|bar| {
            self.pet_unit
                .as_ref()
                .is_some_and(|p| p.actions_usable(bar))
        });
        let Some(bar) = self.pet_bar.as_deref_mut().filter(|_| usable) else {
            return false;
        };
        let pet_guid = bar.spells.pet_guid;
        if let Some(flipped) = crate::ui_pet::toggle_slot_autocast(bar, index + 1) {
            if let Some(c) = &self.commands {
                let _ = c.0.send(crate::net::ClientCommand::PetSetAction {
                    pet_guid,
                    entries: vec![(index, flipped.packed)],
                });
            }
        }
        true
    }

    /// The book's set (`0x5d5e10`) on the lent words: gate, flip, send, in 12340's order.
    fn pet_set_spell_autocast(&mut self, spell_id: u32, state: Option<bool>) -> bool {
        let usable = self.pet_bar.as_deref().is_some_and(|bar| {
            self.pet_unit
                .as_ref()
                .is_some_and(|p| p.actions_usable(bar))
        });
        let Some(bar) = self.pet_bar.as_deref_mut().filter(|_| usable) else {
            return false;
        };
        let Some(enabled) = crate::ui_pet_book::set_autocast(&mut bar.spells, spell_id, state)
        else {
            return false;
        };
        if let Some(c) = &self.commands {
            let _ = c.0.send(crate::net::ClientCommand::PetSpellAutocast {
                pet_guid: bar.spells.pet_guid,
                spell_id,
                enabled,
            });
        }
        true
    }

    /// The pet's book at the call, the array the last `SMSG_PET_SPELLS` filled (`0x5d6cc0`).
    fn pet_book(
        &self,
        anchor: std::time::Instant,
        ui_now: f64,
    ) -> Option<benilla_ui::script::PetBookState> {
        Some(crate::ui_pet_book::book_live(
            self.pet_bar.as_deref()?,
            self.spells.as_deref()?,
            self.classes.as_deref(),
            self.pet_unit.as_ref()?,
            (anchor, ui_now),
        ))
    }

    /// The history's answer for the spell at the call (`0x53e54c` into `0x809000`), with the category and global-cooldown legs its record names.
    fn spell_cooldown(
        &self,
        spell_id: u32,
        anchor: std::time::Instant,
        ui_now: f64,
    ) -> Option<(i64, u32, bool)> {
        let spells = self.spells.as_deref();
        let d = spells.and_then(|s| s.catalog.get(spell_id));
        let cooldowns = self.cooldowns.as_deref()?;
        let triple = cooldowns
            .info(spell_id, 0, d, std::time::Instant::now())
            .ui_triple(anchor, ui_now);
        // The history read's rune leg (`0x807c37`), which needs our player.
        let me = self
            .self_guid
            .as_ref()
            .and_then(|g| g.0)
            .and_then(|g| self.player_entity(g));
        let player = me.and_then(|e| self.stores.get(e).ok()).map(|s| &s.0);
        let (Some(d), Some(spells), Some(runes), Some(costs), Some(mods), Some(player)) = (
            d,
            spells,
            self.runes.as_deref(),
            self.rune_costs.as_deref(),
            self.spell_mods.as_deref(),
            player,
        ) else {
            return triple;
        };
        let leg = crate::spell::cooldowns::RuneLeg {
            runes: crate::spell::usable::RuneCheck {
                state: &runes.state,
                costs: &costs.0,
                player,
            },
            catalog: &spells.catalog,
            mods,
        };
        let now_ms = (ui_now * 1000.0).floor() as i64;
        cooldowns.with_rune_leg(spell_id, d, triple, &leg, now_ms)
    }

    fn player_book(&self) -> Option<std::sync::Arc<benilla_ui::script::PlayerBook>> {
        Some(self.book.as_ref()?.live.clone())
    }

    /// The known-spell bits (`0xbe6d88`), which the learn and removal handlers keep.
    fn spell_known(&self, spell_id: u32) -> bool {
        self.actions
            .as_deref()
            .is_some_and(|a| a.spells.contains(&spell_id))
    }

    /// The known bits, else the higher-rank walk over our race and class's chains (`0x726160`).
    fn spell_known_or_higher(&self, spell_id: u32) -> bool {
        let Some(actions) = self.actions.as_deref() else {
            return false;
        };
        if actions.spells.contains(&spell_id) {
            return true;
        }
        let (Some(lines), Some(me)) = (self.skill_lines.as_deref(), self.player_fields()) else {
            return false;
        };
        lines.catalog.higher_rank_known(
            spell_id,
            me.race_id as u8,
            me.class_id as u8,
            &actions.spells,
        )
    }

    fn book_spell_texture(&self, spell_id: u32) -> Option<String> {
        let spells = self.spells.as_deref()?;
        let d = spells.catalog.get(spell_id)?;
        let me = self
            .player_entity(self.self_guid.as_ref()?.0?)
            .and_then(|e| self.stores.get(e).ok());
        let containers = self.containers.as_ref();
        crate::ui_spellbook::book_texture(
            d,
            || Some(containers?.attack_texture(me?, &spells.forms)),
            || containers?.ranged_texture(me?),
        )
    }

    /// The selected test (`0x53d350`): a trade-skill opener whose line's book is open and not a
    /// linked one (`0x53d3e5`-`0x53d412`), or a shapeshift aura into our current form (`0x53d431`).
    fn spell_selected(&self, spell_id: u32) -> bool {
        let Some(d) = self.spells.as_deref().and_then(|s| s.catalog.get(spell_id)) else {
            return false;
        };
        let Some(me) = self.unit(self.active_player().unwrap_or(0)) else {
            return false;
        };
        if d.effects[0] == benilla_formats::SPELL_EFFECT_TRADE_SKILL {
            let (race, class) = (me.unit_race().unwrap_or(0), me.unit_class().unwrap_or(0));
            let line = self
                .skill_lines
                .as_deref()
                .and_then(|l| l.catalog.spell_to_line(spell_id, race, class));
            if line.is_some() && line == self.trade.as_deref().and_then(|t| t.line) {
                return true;
            }
        }
        let form = me.unit_shapeshift_form();
        form != 0 && d.shapeshift_form == Some(u32::from(form))
    }

    /// The spell tooltip builder at the hover: the player's or its pet's view of the spell, from
    /// the state as it is now; the reference builds it whole at the call and keeps nothing.
    fn spell_tooltip(
        &self,
        spell_id: u32,
        alt_caster: bool,
        get: &dyn Fn(&str) -> Option<String>,
    ) -> Option<benilla_ui::script::SpellTooltipView> {
        use crate::ui_tooltip::spell_view::{build_view, ViewCaster, ViewCtx};
        let spells = self.spells.as_deref()?;
        let (objects, items, commands) = self.containers.as_ref()?.item_reads();
        let me = self.me();
        let store = me.map(|(_, s)| s);
        let units = self.item_range.facts().range_units();
        let no_mods = crate::spell::SpellModifiers::default();
        let operands = crate::spell::text_caster::Operands::of(store, items);
        let mut vctx = ViewCtx {
            home_area: self.tooltip.home_area(),
            form: store.map_or(0, |s| s.0.unit_shapeshift_form()),
            store,
            caster: ViewCaster::Player,
            range_caster: units.caster(),
            attack_target: units.caster_attack_target(),
            objects,
            items,
            commands,
            sub_classes: self.tooltip.sub_classes(),
            skill_lines: self.skill_lines.as_deref().map(|l| &*l.catalog),
            spell_mods: self.spell_mods.as_deref().unwrap_or(&no_mods),
            get,
            known: self
                .actions
                .as_deref()
                .map_or(&crate::ui_spell_info::NO_SPELLS, |a| &a.spells),
            auras: me.and_then(|(e, _)| self.auras.get(e).ok()),
            operands: &operands,
        };
        if alt_caster {
            // The builder's unit selector: the player's charm, else its summon, read as it is now.
            let pet = store
                .and_then(|s| s.0.unit_pet_guid())
                .and_then(|g| self.index.as_ref()?.0.get(&g).copied());
            let pet_fields = pet.and_then(|e| self.stores.get(e).ok()).map(|s| &s.0);
            vctx.caster = ViewCaster::Pet(pet_fields);
            vctx.form = pet_fields.map_or(0, |p| p.unit_shapeshift_form());
            vctx.range_caster =
                pet.and_then(|e| units.unit(e))
                    .unwrap_or(benilla_formats::RangeUnit::still(
                        crate::spell::DEFAULT_REACH,
                    ));
            vctx.attack_target = pet.and_then(|e| units.attack_target(e));
        }
        build_view(spell_id, spells, &mut vctx)
    }

    /// The bar's usable cache (`0x5a7e60`): slot `action - 1` below 144, else nil, nil.
    fn action_current(&self, slot: u32) -> Option<bool> {
        let r = self.resolve_slot(slot)?;
        Some(self.slot_current(r))
    }

    fn action_auto_repeat(&self, slot: u32) -> Option<bool> {
        let r = self.resolve_slot(slot)?;
        Some(self.slot_auto_repeat(r))
    }

    fn action_in_range(&self, slot: u32, target: u64) -> Option<Option<bool>> {
        let r = self.resolve_slot(slot)?;
        Some(self.slot_in_range(r, target))
    }

    fn action_has_range(&self, slot: u32) -> Option<bool> {
        let r = self.resolve_slot(slot)?;
        Some(self.slot_has_range(r))
    }

    fn action_attack(&self, slot: u32) -> Option<bool> {
        let r = self.resolve_slot(slot)?;
        Some(self.slot_attack(r))
    }

    fn action_equipped(&self, slot: u32) -> Option<bool> {
        let r = self.resolve_slot(slot)?;
        Some(self.slot_equipped(r))
    }

    fn action_cooldown(
        &self,
        slot: u32,
        anchor: std::time::Instant,
        ui_now: f64,
    ) -> Option<(i64, u32, bool)> {
        let r = self.resolve_slot(slot)?;
        Some(self.slot_cooldown(r, anchor, ui_now))
    }

    fn item_usable(&self, item_id: u32) -> Option<(bool, bool)> {
        Some(self.item_usable_now(item_id))
    }

    fn temp_action_bar(&self) -> Option<u32> {
        Some(self.pages.as_deref().map_or(0, |p| p.flags))
    }

    fn bonus_bar_offset(&self) -> Option<u8> {
        Some(self.pages.as_deref().map_or(0, |p| p.bonus_offset))
    }

    fn knows_cold_weather_flying(&self) -> Option<bool> {
        Some(self.actions.as_deref().is_some_and(|a| {
            a.spells
                .contains(&crate::spell::usable::COLD_WEATHER_FLYING)
        }))
    }

    fn action_word(&self, slot: u32) -> Option<u32> {
        self.slot_word(slot)
    }

    fn action_texture(&self, slot: u32) -> Option<Option<benilla_ui::script::ActionIcon>> {
        self.slot_icon(slot)
    }

    fn action_count(&self, slot: u32) -> Option<u32> {
        let r = self.resolve_slot(slot)?;
        Some(self.slot_count(r))
    }

    fn action_consumable(&self, slot: u32) -> Option<bool> {
        let r = self.resolve_slot(slot)?;
        Some(self.slot_consumable(r))
    }

    fn action_stackable(&self, slot: u32) -> Option<bool> {
        let r = self.resolve_slot(slot)?;
        Some(self.slot_stackable(r))
    }

    fn action_pet_spell(&self, slot: u32) -> Option<Option<u32>> {
        let r = self.resolve_slot(slot)?;
        Some(self.slot_pet_spell(r))
    }

    fn set_action_word(&mut self, slot: u32, word: u32) -> Option<bool> {
        self.write_slot_word(slot, word)
    }

    fn action_usable(&self, action: u32) -> Option<(bool, bool)> {
        let caches = self.usable.as_deref()?;
        let cell = action
            .checked_sub(1)
            .and_then(|slot| caches.slots.get(slot as usize));
        Some(cell.map_or((false, false), |c| c.pair()))
    }

    /// The stance list (`[0xbe8e2c]`) as `GetShapeshiftFormInfo` reads it (`0x53de90`): the active
    /// test now (`0x802cb0`), castable when active or the cached bytes say so, the active icon
    /// while active.
    fn shapeshift_forms(&self) -> Option<Vec<benilla_ui::script::ShapeshiftFormView>> {
        use crate::ui_spell_info::CallReads;
        let caches = self.usable.as_deref()?;
        let spells = self.spells.as_deref()?;
        let now = self.cast_now();
        let player = self.me().is_some();
        Some(
            caches
                .stances
                .iter()
                .filter_map(|entry| {
                    let d = spells.catalog.get(entry.spell)?;
                    let active = player
                        && crate::ui_spell_info::active_spell(
                            entry.spell,
                            d,
                            &now,
                            &spells.catalog,
                        );
                    let texture = if active {
                        d.active_icon.clone().or_else(|| d.icon.clone())
                    } else {
                        d.icon.clone()
                    };
                    Some(benilla_ui::script::ShapeshiftFormView {
                        spell_id: entry.spell,
                        texture,
                        name: d.name.clone(),
                        active,
                        castable: active || entry.usable.ready(),
                        cooldown: None,
                    })
                })
                .collect(),
        )
    }

    fn shapeshift_form_byte(&self) -> Option<u8> {
        Some(self.me().map_or(0, |(_, s)| s.0.unit_shapeshift_form()))
    }

    fn with_spell_source(&self, f: &mut dyn FnMut(&dyn benilla_ui::script::SpellInfoSource)) {
        let Some(spells) = self.spells.as_deref() else {
            return;
        };
        f(&crate::ui_spell_info::Answers {
            tables: crate::ui_spell_info::Tables {
                spells,
                skill_lines: self.skill_lines.as_deref().map(|l| &*l.catalog),
                talents: self.talents.as_deref().map(|t| &*t.catalog),
            },
            reads: self,
        });
    }

    fn spell_targeting(&self) -> benilla_ui::script::SpellTargetingLive {
        let Some(t) = self.cast.targeting.as_deref() else {
            return Default::default();
        };
        benilla_ui::script::SpellTargetingLive {
            active: t.active(),
            wants_item: t.wants(crate::spell::targeting::TargetingWants::Item),
            arms: t.arms(),
        }
    }

    /// The target check's unit leg toward the held unit `guid` at the call (`0x804190`): the
    /// standing word binds it and no range refusal stands.
    fn spell_can_target_unit(&self, guid: u64) -> bool {
        let (Some(t), Some(bind)) = (self.cast.targeting.as_deref(), self.bind.as_ref()) else {
            return false;
        };
        let Some(&entity) = self.index.as_ref().and_then(|i| i.0.get(&guid)) else {
            return false;
        };
        t.can_target_unit(entity, bind)
    }

    fn totem(&self, slot: usize) -> Option<benilla_ui::script::TotemView> {
        self.totems.slot(slot)
    }

    fn totem_tool(&self, slot: usize) -> bool {
        self.totems.tool(slot)
    }

    fn destroy_totem(&mut self, slot: usize) -> bool {
        self.totems.destroy(slot)
    }

    fn multicast_list(&self, element: usize) -> Vec<u32> {
        self.totems.list(element)
    }

    fn spell_totem_categories(&self, spell: u32) -> Option<[u32; 2]> {
        self.totems.spell_categories(spell)
    }

    fn totem_category_name(&self, row: u32) -> Option<String> {
        self.totems.category_name(row)
    }

    fn unit_cast(&self, guid: u64, channel: bool) -> Option<benilla_ui::script::UnitCast> {
        let casts = self.cast.unit_casts.as_deref()?;
        let schools = self.schools.as_deref().copied().unwrap_or_default();
        casts.live_cast(guid, channel, self.spells.as_deref(), schools)
    }

    /// The auto-repeating spell, else the in-flight slot the cancel resolves (`0x7fe130`,
    /// `0x805f30`), as `SpellStopCasting` tests them.
    fn has_stoppable_cast(&self) -> bool {
        if self
            .cast
            .auto_repeat
            .as_deref()
            .is_some_and(|a| a.0.is_some())
        {
            return true;
        }
        let (Some(pending), Some(melee)) = (
            self.cast.pending.as_deref(),
            self.cast.queued_melee.as_deref(),
        ) else {
            return false;
        };
        let started = self
            .self_guid
            .as_ref()
            .and_then(|g| g.0)
            .and_then(|g| self.index.as_ref()?.0.get(&g).copied())
            .and_then(|e| self.casting.get(e).ok())
            .map(|c| c.spell_id);
        crate::spell::inflight(pending, melee, started).is_some()
    }

    /// The rune table and our `PLAYER_RUNE_REGEN` rates now, behind the player-mask lookup of the
    /// active player every rune verb opens with (`0x61306f`).
    fn runes(&self) -> Option<(benilla_ui::script::RuneState, benilla_ui::script::RuneRegen)> {
        let me = self.player_entity(self.self_guid.as_ref()?.0?)?;
        let fields = &self.stores.get(me).ok()?.0;
        let table = self
            .runes
            .as_ref()
            .map(|r| r.state.clone())
            .unwrap_or_default();
        Some((table, crate::ui_rune::regen_of(fields)))
    }

    fn banker(&self) -> Option<u64> {
        self.lent.bank.as_deref()?.banker
    }

    fn bank_bag_slots(&self) -> Option<u8> {
        Some(
            self.unit(self.active_player()?)?
                .player_bank_bag_slots_purchased()
                .unwrap_or(0),
        )
    }

    fn bank_slot_cost(&self, slot: u32) -> u32 {
        let row = u8::try_from(slot.saturating_sub(1)).unwrap_or(u8::MAX);
        self.bank_prices
            .as_ref()
            .and_then(|p| p.0.next_slot_price(row))
            .unwrap_or(0)
    }

    /// The active player, control, a banker and fewer than seven slots gate silently (`0x57bbe5`-`0x57bc10`); short of the price shows `ERR_BANKSLOT_INSUFFICIENT_FUNDS` (`0x57bc2a`); else `CMSG_BUY_BANK_SLOT` names the interact target (`0x57bc54`).
    fn purchase_bank_slot(&self) -> Option<&'static str> {
        let me = self.unit(self.active_player()?)?;
        let bank = self.lent.bank.as_deref()?;
        bank.banker.filter(|_| self.has_control())?;
        let slots = me.player_bank_bag_slots_purchased().unwrap_or(0);
        if slots >= 7 {
            return None;
        }
        if me.player_money().unwrap_or(0) < self.bank_slot_cost(u32::from(slots) + 1) {
            return Some("ERR_BANKSLOT_INSUFFICIENT_FUNDS");
        }
        let guid = self.tabard.target().unwrap_or(0);
        debug!("bank: purchase next bag slot at {guid:#x}");
        let _ = self
            .net
            .as_deref()?
            .0
            .send(crate::net::ClientCommand::BuyBankSlot { guid });
        None
    }

    fn close_bank_interaction(&mut self) -> bool {
        let banker = self.lent.bank.as_deref().and_then(|b| b.banker);
        match (banker, self.tabard.target_mut()) {
            (Some(banker), Some(t)) => t.clear_in_call(banker),
            _ => false,
        }
    }

    fn clear_interact_target(&mut self, guid: u64) -> bool {
        self.tabard
            .target_mut()
            .is_some_and(|t| t.clear_in_call(guid))
    }

    fn item_tables(&self) -> Option<&benilla_ui::script::ItemTables> {
        self.item_tables.as_deref().map(|t| &t.tables)
    }

    fn item_set(
        &self,
        set_id: u32,
        global: &dyn Fn(&str) -> Option<String>,
    ) -> Option<benilla_ui::script::ItemSetView> {
        let me = self.self_guid.as_ref().and_then(|s| s.0).and_then(|g| {
            let &entity = self.index.as_ref()?.0.get(&g)?;
            self.stores.get(entity).ok()
        });
        let known = self.actions.as_deref().map(|a| &a.spells);
        static NONE: std::collections::BTreeSet<u32> = std::collections::BTreeSet::new();
        self.item_sets.as_ref()?.view(
            set_id,
            me,
            self.spells.as_deref(),
            known.unwrap_or(&NONE),
            global,
        )
    }

    fn item_record(
        &self,
        id: u32,
        tooltip: Option<benilla_ui::script::GlobalStrings<'_>>,
    ) -> Option<Option<benilla_ui::script::ItemTemplateView>> {
        let records = self.item_records.as_ref()?;
        let me = self.self_guid.as_ref().and_then(|s| s.0).and_then(|g| {
            let &entity = self.index.as_ref()?.0.get(&g)?;
            self.stores.get(entity).ok()
        });
        let known = self.actions.as_deref().map(|a| &a.spells);
        static NONE: std::collections::BTreeSet<u32> = std::collections::BTreeSet::new();
        Some(records.view(
            id,
            me,
            self.spells.as_deref(),
            known.unwrap_or(&NONE),
            tooltip,
        ))
    }

    fn bind_location(&self) -> Option<String> {
        self.item_records
            .as_ref()?
            .bind_location()
            .map(str::to_string)
    }

    fn guild_bank_open(&self) -> bool {
        self.lent.guild_bank.as_deref().is_some_and(|b| b.is_open())
    }

    fn guild_bank_current_tab(&self) -> u8 {
        self.lent
            .guild_bank
            .as_deref()
            .map_or(0, |b| b.current_tab())
    }

    fn set_guild_bank_current_tab(&mut self, tab: u8) {
        if let Some(bank) = self.lent.guild_bank.as_deref_mut() {
            bank.set_current_tab(tab);
        }
    }

    fn guild_bank_num_tabs(&self) -> u8 {
        self.lent.guild_bank.as_deref().map_or(0, |b| b.num_tabs())
    }

    fn guild_bank_tab(&self, tab: u8) -> Option<benilla_ui::script::GuildBankTabView> {
        self.lent.guild_bank.as_deref()?.tab_view(tab)
    }

    fn guild_bank_item(&self, tab: u8, slot: u8) -> Option<(u32, u32)> {
        let s = self.lent.guild_bank.as_deref()?.slot(tab, slot)?;
        Some((s.item_id, s.count))
    }

    fn guild_bank_slot(&self, tab: u8, slot: u8) -> Option<benilla_ui::script::GuildBankSlotView> {
        let s = self.lent.guild_bank.as_deref()?.slot(tab, slot)?;
        self.guild_bank
            .as_ref()?
            .slot_view(s, self.commands.as_deref()?)
    }

    fn guild_bank_texture(&self, tab: u8, slot: u8) -> Option<String> {
        let s = self.lent.guild_bank.as_deref()?.slot(tab, slot)?;
        self.guild_bank.as_ref()?.texture(s.item_id)
    }

    fn guild_bank_locked(&self, tab: u8, slot: u8, item_id: u32) -> bool {
        self.lent
            .guild_bank
            .as_deref()
            .is_some_and(|b| b.is_locked(tab, slot, item_id))
    }

    fn set_guild_bank_lock(&mut self, tab: u8, slot: u8, item_id: u32, locked: bool) {
        if let Some(bank) = self.lent.guild_bank.as_deref_mut() {
            bank.set_lock(tab, slot, item_id, locked);
        }
    }

    fn guild_bank_withdrawals(&self) -> i32 {
        self.lent
            .guild_bank
            .as_deref()
            .map_or(0, |b| b.withdrawals_remaining())
    }

    fn guild_bank_money(&self) -> u64 {
        self.lent.guild_bank.as_deref().map_or(0, |b| b.money())
    }

    fn guild_bank_withdraw_money(&self) -> i32 {
        self.lent
            .guild_bank
            .as_deref()
            .map_or(0, |b| b.withdraw_money())
    }

    fn guild_bank_log_len(&self, tab: u8) -> usize {
        self.lent
            .guild_bank
            .as_deref()
            .map_or(0, |b| b.log_len(tab))
    }

    fn guild_bank_log(&self, tab: u8, index: u8) -> Option<benilla_ui::script::GuildBankLogView> {
        let row = self.lent.guild_bank.as_deref()?.log_row(tab, index)?;
        Some(
            self.guild_bank
                .as_ref()?
                .log_view(row, self.commands.as_deref()?),
        )
    }

    fn clear_guild_banker(&mut self) {
        if let Some(bank) = self.lent.guild_bank.as_deref_mut() {
            bank.clear_banker();
        }
    }

    fn guild_bank_query_text(&mut self, tab: u8) -> bool {
        let commands = self.commands.as_deref();
        match (self.lent.guild_bank.as_deref_mut(), commands) {
            (Some(bank), Some(net)) => bank.query_text(tab, net),
            _ => true,
        }
    }

    fn player_fields(&self) -> Option<benilla_ui::script::PlayerFields> {
        let me = self.unit(self.active_player()?)?;
        Some(crate::ui_items::player_fields(me, self.classes.as_deref()))
    }

    fn player_skill(&self, skill: u32) -> Option<Option<u32>> {
        let me = self.unit(self.active_player()?)?;
        Some(crate::ui_items::player_skill(me, skill))
    }

    fn player_proficiency(&self, class: u32) -> Option<Option<u32>> {
        self.unit(self.active_player()?)?;
        Some(
            self.proficiencies
                .as_deref()
                .and_then(|p| p.0.get(&class).copied()),
        )
    }

    fn player_rep_rank(&self, faction: u32) -> Option<u8> {
        let me = self.unit(self.active_player()?)?;
        Some(crate::ui_items::player_rep_rank(
            me,
            self.reputations.as_deref(),
            self.factions.as_deref(),
            faction,
        ))
    }

    fn player_can_dual_wield(&self) -> Option<bool> {
        self.unit(self.active_player()?)?;
        Some(crate::ui_items::can_dual_wield(
            self.actions.as_deref(),
            self.spells.as_deref(),
        ))
    }

    fn player_level(&self) -> Option<u32> {
        let me = self.unit(self.active_player()?)?;
        Some(me.unit_level().unwrap_or(0))
    }

    fn container(
        &self,
        bag: i64,
        anchor: std::time::Instant,
        ui_now: f64,
    ) -> Option<Option<benilla_ui::script::ContainerState>> {
        self.containers.as_ref()?.bag(
            &self.container_lent(),
            bag,
            crate::ui_items::BagRead::Held,
            (anchor, ui_now),
        )
    }

    fn container_slot(
        &self,
        bag: i64,
        slot: u32,
        anchor: std::time::Instant,
        ui_now: f64,
    ) -> Option<Option<benilla_ui::script::ContainerSlot>> {
        self.containers.as_ref()?.slot(
            &self.container_lent(),
            bag,
            slot,
            crate::ui_items::BagRead::Held,
            (anchor, ui_now),
        )
    }

    fn indexed_container(
        &self,
        bag: i64,
        anchor: std::time::Instant,
        ui_now: f64,
    ) -> Option<Option<benilla_ui::script::ContainerState>> {
        self.containers.as_ref()?.bag(
            &self.container_lent(),
            bag,
            crate::ui_items::BagRead::Index,
            (anchor, ui_now),
        )
    }

    fn indexed_container_slot(
        &self,
        bag: i64,
        slot: u32,
        anchor: std::time::Instant,
        ui_now: f64,
    ) -> Option<Option<benilla_ui::script::ContainerSlot>> {
        self.containers.as_ref()?.slot(
            &self.container_lent(),
            bag,
            slot,
            crate::ui_items::BagRead::Index,
            (anchor, ui_now),
        )
    }

    fn has_key(&self) -> Option<bool> {
        self.containers.as_ref()?.has_key()
    }

    fn currency_catalog(&self) -> Option<&benilla_ui::script::CurrencyCatalogView> {
        self.currency.catalog()
    }

    fn currency_holdings(&self) -> Option<benilla_ui::script::CurrencyHoldings> {
        let me = self.unit(self.active_player()?)?;
        let index = self.index.as_ref()?;
        Some(self.currency.holdings(me, |guid| {
            let &entity = index.0.get(&guid)?;
            Some(&self.stores.get(entity).ok()?.0)
        }))
    }

    fn ask_item_record(&self, id: u32) {
        self.currency.ask(id);
    }

    fn wait_item_record(&mut self, id: u32, wait: benilla_ui::script::ItemRecordWait) -> bool {
        let Some(waits) = self.record_waits.as_mut() else {
            return false;
        };
        waits.wait(id, crate::items::RecordWait::Ui(wait), wait.once());
        true
    }

    fn clear_banker(&mut self) {
        if let Some(b) = self.lent.bank.as_deref_mut() {
            b.clear();
        }
    }

    fn refund_record(&self, item: u64) -> Option<Option<benilla_ui::script::RefundView>> {
        let records = self.lent.refunds.as_deref()?;
        let held = self.index.as_ref()?.0.contains_key(&item);
        Some(held.then(|| records.view(item)).flatten())
    }

    fn bound_trade_stamp(&self, item: u64) -> Option<Option<u32>> {
        let index = self.index.as_ref()?;
        let fields = index
            .0
            .get(&item)
            .and_then(|&e| self.stores.get(e).ok())
            .map(|s| &s.0);
        let me = self.active_player();
        Some(fields.and_then(|f| crate::ui_socket::trade_stamp(f, me, self.enchants.as_deref())))
    }

    fn ask_refund_record(&mut self, item: u64) {
        let entry = self
            .index
            .as_ref()
            .and_then(|i| i.0.get(&item))
            .and_then(|&e| self.stores.get(e).ok())
            .and_then(|s| s.0.object_entry());
        let refundable = entry
            .zip(self.items.as_deref())
            .is_some_and(|(entry, items)| crate::ui_socket::refundable(items, entry));
        let Some(commands) = self.commands.as_deref() else {
            return;
        };
        if let (true, Some(records)) = (refundable, self.lent.refunds.as_deref_mut()) {
            records.ask(item, commands);
        }
    }

    fn binder_in_reach(&self) -> bool {
        self.talk.binder_in_reach()
    }

    fn confirm_binder(&self) {
        self.talk.confirm_binder();
    }

    fn talent_master_in_reach(&self) -> bool {
        self.talk.talent_master_in_reach()
    }

    fn confirm_talent_wipe(&self) -> Option<&'static str> {
        self.talk.confirm_talent_wipe()
    }

    fn instance_boot_secs(&self) -> u32 {
        self.dialogs.instance_boot_secs()
    }

    fn area_spirit_healer_secs(&self) -> u32 {
        self.lent
            .spirit
            .as_deref()
            .map_or(0, |s| s.secs(std::time::Instant::now()))
    }

    fn area_spirit_poll_drop(&mut self) -> Option<bool> {
        let spirit = self.lent.spirit.as_deref_mut()?;
        Some(crate::ui_dialog_verbs::poll_drop(
            spirit,
            &self.dialogs.spirit,
        ))
    }

    fn accept_area_spirit_heal(&mut self, cancelled: bool) {
        if let Some(spirit) = self.lent.spirit.as_deref_mut() {
            self.dialogs.accept(spirit, cancelled);
        }
    }

    fn spirit_cancel_has_body(&self) -> bool {
        self.dialogs.has_body()
    }

    fn cancel_area_spirit_heal(&self) {
        self.dialogs.cancel_area_spirit_heal();
    }

    fn active_player_held(&self) -> bool {
        self.self_guid
            .as_ref()
            .and_then(|g| g.0)
            .is_some_and(|g| self.player_entity(g).is_some())
    }

    fn tabard_guild_record(&self) -> Option<[i32; 5]> {
        self.tabard.guild_record()
    }

    fn tabard_save_pending(&self) -> bool {
        self.tabard.save_pending()
    }

    fn save_tabard(&mut self, design: [i32; 5]) -> Option<Result<(), &'static str>> {
        self.tabard.save(design)
    }

    fn close_tabard(&mut self) -> u32 {
        self.tabard.close()
    }

    fn dress_up(
        &mut self,
        pane: benilla_ui::widget::FrameHandle,
        intent: benilla_ui::script::DressUpIntent,
    ) {
        let waits = self.record_waits.as_deref_mut();
        self.dressup.dress_up(pane.bits(), intent, waits);
    }

    fn barber(&self) -> Option<benilla_ui::script::BarberRead<'_>> {
        self.barber.read()
    }

    fn gossip(&self) -> Option<benilla_ui::script::GossipRead<'_>> {
        self.gossip.read()
    }

    fn select_gossip_option(&mut self, slot: usize, code: Option<String>) {
        self.gossip.select_option(slot, code);
    }

    fn select_gossip_quest(&mut self, row: usize) {
        self.gossip.select_quest(row);
    }

    fn item_text(&self) -> Option<benilla_ui::script::ItemTextRead> {
        self.item_text.read()
    }

    fn item_text_creator(&mut self) -> Option<String> {
        self.item_text.creator()
    }

    fn item_text_turn(&mut self, next: bool) -> bool {
        self.item_text.turn(next, self.actions.as_deref())
    }

    fn close_item_text(&mut self) -> u32 {
        self.item_text.close(self.tabard.target_mut())
    }

    fn player_money(&self) -> Option<u64> {
        let me = self.active_player().and_then(|g| self.unit(g));
        Some(u64::from(me.and_then(|m| m.player_money()).unwrap_or(0)))
    }

    fn merchant_open(&self) -> Option<bool> {
        Some(self.merchant.as_ref()?.open())
    }

    fn merchant_len(&self, list: benilla_ui::script::MerchantList) -> Option<usize> {
        Some(self.merchant.as_ref()?.len(list))
    }

    fn merchant_row(
        &mut self,
        list: benilla_ui::script::MerchantList,
        index: usize,
        ask: bool,
    ) -> Option<Option<benilla_ui::script::MerchantItem>> {
        let waits = self.record_waits.as_deref_mut();
        Some(self.merchant.as_mut()?.row(list, index, ask, waits))
    }

    fn can_merchant_repair(&self) -> Option<bool> {
        Some(self.merchant.as_ref()?.can_repair())
    }

    fn repair_all_cost(&self) -> Option<Option<(u32, bool)>> {
        Some(self.merchant.as_ref()?.repair_all_cost())
    }

    fn repair_cost(&self, at: benilla_ui::script::RepairAt) -> Option<u32> {
        Some(self.merchant.as_ref()?.repair_cost(at))
    }

    fn merchant_act(
        &mut self,
        act: benilla_ui::script::MerchantAct,
    ) -> Option<benilla_ui::script::MerchantOutcome> {
        let target = self.tabard.target_mut();
        Some(self.merchant.as_mut()?.act(act, target))
    }

    fn taxi_map(&self) -> Option<Option<&benilla_ui::script::TaxiUiState>> {
        Some(self.taxi.as_ref()?.map())
    }

    fn taxi_node_cost(&self, slot: u32) -> Option<i64> {
        Some(self.taxi.as_ref()?.cost(slot))
    }

    fn taxi_act(
        &mut self,
        act: benilla_ui::script::TaxiAct,
    ) -> Option<benilla_ui::script::TaxiOutcome> {
        let target = self.tabard.target_mut();
        Some(self.taxi.as_mut()?.act(act, target))
    }

    fn petition_view(&self) -> Option<benilla_ui::script::PetitionState> {
        Some(self.petition.as_ref()?.view())
    }

    fn petition_vendor_open(&self) -> Option<bool> {
        Some(self.petition.as_ref()?.vendor_open())
    }

    fn petition_vendor_count(&self) -> Option<usize> {
        Some(self.petition.as_ref()?.vendor_count())
    }

    fn petition_item(
        &mut self,
        index: u32,
    ) -> Option<Option<benilla_ui::script::PetitionVendorItem>> {
        let waits = self.record_waits.as_deref_mut();
        Some(self.petition.as_ref()?.item(index, waits))
    }

    fn petition_act(
        &mut self,
        act: benilla_ui::script::PetitionRequest,
    ) -> Option<benilla_ui::script::PetitionOutcome> {
        let target = self.selection.as_deref().and_then(|s| s.guid);
        let designer = self.tabard.vendor();
        let interact = self.tabard.target_mut();
        let cx = crate::ui_petition::ActContext {
            target,
            designer,
            interact,
        };
        Some(self.petition.as_mut()?.act(act, cx))
    }

    fn trainer(&self) -> Option<Option<&benilla_ui::script::TrainerWindow>> {
        Some(Some(self.trainer.as_ref()?.window()))
    }

    fn trainer_mut(&mut self) -> Option<Option<&mut benilla_ui::script::TrainerWindow>> {
        Some(Some(self.trainer.as_mut()?.window_mut()))
    }

    fn trainer_service_live(
        &self,
        position: usize,
    ) -> Option<Option<benilla_ui::script::TrainerServiceLive>> {
        let empty = std::collections::BTreeSet::new();
        let known = self.actions.as_deref().map_or(&empty, |a| &a.spells);
        Some(self.trainer.as_ref()?.service_live(position, known))
    }

    fn trainer_description(
        &mut self,
        position: usize,
        global: benilla_ui::script::GlobalStrings<'_>,
    ) -> Option<Option<String>> {
        let empty = std::collections::BTreeSet::new();
        let known = self.actions.as_deref().map_or(&empty, |a| &a.spells);
        let waits = self.record_waits.as_deref_mut();
        Some(
            self.trainer
                .as_ref()?
                .description(position, known, waits, global),
        )
    }

    fn trainer_buy(&mut self, spell_id: u32) -> bool {
        let Some(t) = self.trainer.as_ref() else {
            return false;
        };
        t.buy(spell_id);
        true
    }

    fn trainer_open(&self) -> Option<bool> {
        Some(self.trainer.as_ref()?.is_open())
    }

    fn trainer_release_target(&mut self) -> bool {
        let target = self.tabard.target_mut();
        self.trainer
            .as_mut()
            .is_some_and(|t| t.release_target(target))
    }

    fn trainer_zero(&mut self) {
        if let Some(t) = self.trainer.as_mut() {
            t.zero();
        }
    }

    fn close_gossip(&mut self) -> u32 {
        use crate::ui_session::NpcSession;
        let giver_held = self.lent.giver.as_deref().and_then(|q| q.npc()).is_some();
        self.gossip.close(self.tabard.target_mut(), giver_held)
    }

    fn set_barber_dials(&mut self, dials: [u8; 5]) {
        self.barber.set_dials(dials);
    }

    fn set_barber_skin_style(&mut self, data: u8) {
        self.barber.set_skin_style(data);
    }

    fn send_alter_appearance(&mut self, styles: [u32; 4]) {
        self.barber.send(styles);
    }

    fn close_barber_shop(&mut self) -> bool {
        self.barber.close()
    }

    fn hair_customization(&self) -> Option<&str> {
        self.barber.hair_customization()
    }

    fn facial_hair_customization(&self) -> Option<&str> {
        self.barber.facial_hair_customization()
    }

    fn apply_guild_color(&mut self, design: [i32; 5]) {
        self.tabard.apply_guild_color(design);
    }
}

impl MainState<'_> {
    /// The rebuild's writes, `None` while any is missing.
    fn row_writes(&mut self) -> Option<crate::ui_quest_log::rows::RowWrites<'_>> {
        let Self {
            signals,
            marks,
            expiries,
            ..
        } = self;
        Some(crate::ui_quest_log::rows::RowWrites {
            marks: marks.as_deref_mut()?,
            expiries: expiries.as_deref_mut()?,
            signals: signals.as_deref_mut()?,
        })
    }
}

/// The interact target (`0xbd07a8`) through the `World` lent to the running call, `None` outside one.
#[cfg(test)]
pub(crate) fn interact_target() -> Option<Option<u64>> {
    benilla_ui::script::read_live(|l| l.interact_target())
}

/// A handler's write to state the VM owns, run in turn with the events around it.
type VmWrite = Box<dyn FnOnce(&mut UiScript) + Send + Sync>;

/// A step that needs the whole `World`, run in turn with the events around it.
type WorldStep = Box<dyn FnOnce(&mut World) + Send + Sync>;

/// One queued call into the VM.
enum UiEvent {
    Fire(&'static str, Vec<ScriptValue>),
    /// The cursor clear a handler runs beside its event (`0x520fab`).
    ClearCursor,
    /// A write to what the VM owns, such as the battlefield list a list packet fills (`0x54e390`).
    Write(VmWrite),
    /// A system chat line, its template read off the VM and filled (`0x5498f6`, `0x5cfb20`).
    SystemLine(&'static str, Vec<LineArg>),
    /// A step over the `World`, such as the unit pushes an event's tokens read.
    World(WorldStep),
    /// One cast or channel edge applied to the caster's cast state, its events fired in turn.
    CastEdge(crate::ui_cast::CastBarEdge),
}

/// An argument a system line's template takes.
pub(crate) enum LineArg {
    S(String),
    D(i64),
}

/// The events this frame's handlers fired, in order; the drain empties it after each handler.
#[derive(Resource, Default)]
pub(crate) struct UiEvents(Vec<UiEvent>);

impl UiEvents {
    /// What waits now, taken: an applier's own raises then queue and fire apart from it.
    pub(crate) fn set_aside(&mut self) -> Self {
        Self(std::mem::take(&mut self.0))
    }

    /// Put what [`Self::set_aside`] took back ahead of anything queued since.
    pub(crate) fn put_back(&mut self, earlier: Self) {
        let later = std::mem::replace(&mut self.0, earlier.0);
        self.0.extend(later);
    }

    /// Nothing waits.
    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Fire `event` once the calling handler's changes have applied, before the next handler runs.
    pub(crate) fn fire(&mut self, event: &'static str, args: Vec<ScriptValue>) {
        self.0.push(UiEvent::Fire(event, args));
    }

    /// Clear the cursor in turn with the events around it.
    pub(crate) fn clear_cursor(&mut self) {
        self.0.push(UiEvent::ClearCursor);
    }

    /// Add the system chat line `key`'s template names, filled with `args` as the handler runs.
    pub(crate) fn system_line(&mut self, key: &'static str, args: Vec<LineArg>) {
        self.0.push(UiEvent::SystemLine(key, args));
    }

    /// Write state the VM owns in turn with the events around it; `write` may fire its own.
    pub(crate) fn write(&mut self, write: impl FnOnce(&mut UiScript) + Send + Sync + 'static) {
        self.0.push(UiEvent::Write(Box::new(write)));
    }

    /// Run `step` over the `World` in turn with the events around it; what it queues fires next.
    pub(crate) fn world(&mut self, step: impl FnOnce(&mut World) + Send + Sync + 'static) {
        self.0.push(UiEvent::World(Box::new(step)));
    }

    /// Apply a cast or channel edge to its caster in turn with the events around it, as the
    /// handler or press that made it does; its `UNIT_SPELLCAST_*` events fire next.
    pub(crate) fn cast_edge(&mut self, edge: crate::ui_cast::CastBarEdge) {
        self.0.push(UiEvent::CastEdge(edge));
    }

    /// The cast refusals queued, taken.
    #[cfg(test)]
    pub(crate) fn take_refusals(&mut self) -> Vec<crate::ui_action::CastFail> {
        let mut out = Vec::new();
        self.0.retain(|e| match e {
            UiEvent::CastEdge(crate::ui_cast::CastBarEdge::Refused(f)) => {
                out.push(*f);
                false
            }
            _ => true,
        });
        out
    }

    /// The press steps queued (a send's rest, an attempt's rest), taken in order.
    #[cfg(test)]
    pub(crate) fn take_press_steps(&mut self) -> Vec<crate::ui_cast::CastBarEdge> {
        let mut out = Vec::new();
        self.0.retain(|e| match e {
            UiEvent::CastEdge(edge) if edge.is_step() => {
                out.push(edge.clone());
                false
            }
            _ => true,
        });
        out
    }

    /// The cast edges queued, in order.
    #[cfg(test)]
    pub(crate) fn cast_edges(&self) -> Vec<crate::ui_cast::CastBarEdge> {
        self.0
            .iter()
            .filter_map(|e| match e {
                UiEvent::CastEdge(edge) => Some(edge.clone()),
                _ => None,
            })
            .collect()
    }

    /// The fired event names, in order.
    #[cfg(test)]
    pub(crate) fn names(&self) -> Vec<&'static str> {
        self.0
            .iter()
            .map(|e| match e {
                UiEvent::Fire(name, _) => *name,
                UiEvent::ClearCursor => "<clear cursor>",
                UiEvent::Write(_) => "<write>",
                UiEvent::SystemLine(key, _) => key,
                UiEvent::World(_) => "<world>",
                UiEvent::CastEdge(_) => "<cast edge>",
            })
            .collect()
    }
}

/// What [`with_world_view`] lends: the view as a system lends it, owning what it may write.
/// The cached parameter state [`with_world_view`] builds the view from.
#[derive(Resource)]
pub(super) struct LiveViewState(SystemState<LiveView<'static, 'static>>);

/// The view's state taken out of the world for a read, put back on return and on unwind.
struct HeldState<'w> {
    world: &'w mut World,
    state: Option<LiveViewState>,
}

impl Drop for HeldState<'_> {
    fn drop(&mut self) {
        if let Some(state) = self.state.take() {
            self.world.insert_resource(state);
        }
    }
}

/// Run `f` on the VM with the `World` lent to every verb it reaches; `None` with no VM.
pub(crate) fn with_live_world<R>(
    world: &mut World,
    f: impl FnOnce(&mut UiScript) -> R,
) -> Option<R> {
    super::safe_world::lend(world, f)
}

/// Run `f` on `script`, a VM held outside the world, with the `World` lent.
pub(crate) fn lend_world<R>(
    world: &mut World,
    script: &mut UiScript,
    f: impl FnOnce(&mut UiScript) -> R,
) -> R {
    super::safe_world::lend_to(world, script, f)
}

/// `f` over the `World`'s live view, built over the `World` it borrows for `f` alone: one read or
/// write a verb makes through the `World` loan ([`super::safe_world`]).
pub(crate) fn with_world_view<R>(world: &mut World, f: impl FnOnce(&mut LiveView) -> R) -> R {
    #[cfg(test)]
    VIEW_BUILDS.with(|n| n.set(n.get() + 1));
    let state = match world.remove_resource::<LiveViewState>() {
        Some(state) => state,
        None => LiveViewState(SystemState::new(world)),
    };
    let mut held = HeldState {
        world,
        state: Some(state),
    };
    let state = held.state.as_mut().expect("held until the drop");
    let mut view = state.0.get_mut(held.world);
    f(&mut view)
}

#[cfg(test)]
thread_local! {
    /// How many views [`with_world_view`] built on this thread, for the cost tests.
    pub(crate) static VIEW_BUILDS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// `step` in turn after the events queued so far, or now with no event queue.
pub(crate) fn in_turn(world: &mut World, step: impl FnOnce(&mut World) + Send + Sync + 'static) {
    match world.get_resource_mut::<UiEvents>() {
        Some(mut ui) => ui.world(step),
        None => step(world),
    }
}

/// Fires what the handler that just ran queued, in its order, with the `World` lent; it runs after
/// each handler, not after each packet. With no VM they are dropped, as the reference has no
/// listener before its UI loads.
pub(crate) fn fire_handler_events(world: &mut World) {
    let Some(mut queued) = world.get_resource_mut::<UiEvents>() else {
        return;
    };
    if queued.0.is_empty() {
        return;
    }
    let mut events = std::collections::VecDeque::from(std::mem::take(&mut queued.0));
    let mut lines = Vec::new();
    let mut batch = Vec::new();
    while let Some(event) = events.pop_front() {
        match event {
            step @ (UiEvent::World(_) | UiEvent::CastEdge(_)) => {
                fire_batch(world, &mut batch, &mut lines);
                match step {
                    UiEvent::World(step) => step(world),
                    UiEvent::CastEdge(edge) => crate::ui_cast::apply_edge(world, edge),
                    _ => unreachable!(),
                }
                // What the step queued fires in its turn, ahead of the rest.
                if let Some(mut queued) = world.get_resource_mut::<UiEvents>() {
                    for e in std::mem::take(&mut queued.0).into_iter().rev() {
                        events.push_front(e);
                    }
                }
            }
            other => batch.push(other),
        }
    }
    fire_batch(world, &mut batch, &mut lines);
    // The chat log carries the line to the chat frame, as the chat feed's other system lines go.
    if let Some(mut chat) = world.get_resource_mut::<crate::ui_chat::ChatLog>() {
        for line in lines {
            crate::ui_chat::away::push_system(&mut chat, line);
        }
    }
}

/// Fire `batch` into the VM, in order, with the `World` lent, collecting the system lines.
fn fire_batch(world: &mut World, batch: &mut Vec<UiEvent>, lines: &mut Vec<String>) {
    if batch.is_empty() {
        return;
    }
    let events = std::mem::take(batch);
    with_live_world(world, |script| {
        for event in events {
            match event {
                UiEvent::Fire(event, args) => script.fire_event(event, args),
                UiEvent::ClearCursor => script.clear_cursor_payload(),
                UiEvent::Write(write) => write(script),
                UiEvent::SystemLine(key, args) => {
                    use benilla_ui::strings::Arg;
                    let args: Vec<Arg> = args
                        .iter()
                        .map(|a| match a {
                            LineArg::S(s) => Arg::S(s),
                            LineArg::D(d) => Arg::D(*d),
                        })
                        .collect();
                    let text = crate::ui_chat::combat::global_string(script, key)
                        .map(|t| benilla_ui::strings::fill(&t, &args));
                    lines.extend(text.filter(|t| !t.is_empty()));
                }
                // Split out by the caller.
                UiEvent::World(_) | UiEvent::CastEdge(_) => {}
            }
        }
    });
}

/// The unique signals queued this frame: each event once, however often it is queued (`0x6143f0`), fired in queue order once per frame after the packets (`0x52b247` -> `0x614760`). Main-thread state the handlers fill and a verb's call queues into ([`Lent`]).
#[derive(Default)]
pub(crate) struct UniqueSignals(Vec<&'static str>);

impl UniqueSignals {
    /// Queue `event` for the frame's flush unless it already waits (`0x615020`).
    pub(crate) fn unique_signal(&mut self, event: &'static str) {
        if !self.0.contains(&event) {
            self.0.push(event);
        }
    }

    /// The game UI's shutdown empties the queue (`0x52902f`), so nothing waits for the next login.
    pub(crate) fn clear(&mut self) {
        self.0.clear();
    }

    /// The queued event names, in order.
    #[cfg(test)]
    pub(crate) fn names(&self) -> Vec<&'static str> {
        self.0.clone()
    }
}

/// The game UI's idle tail (`0x52af40`): the watch timers run (`0x52b229`), then the frame's unique signals fire in queue order without arguments (`0x52b247` -> `0x614760`), after the feeds that queue them; with no VM they are dropped.
pub(crate) fn flush_unique_signals(world: &mut World) {
    if world.get_non_send_resource::<UiScript>().is_none() {
        if let Some(mut signals) = world.get_non_send_resource_mut::<UniqueSignals>() {
            signals.clear();
        }
        return;
    }
    super::safe_world::lend(world, |script| script.update_quest_watch_timers());
    // The count taken at entry, its entries still queued while they fire, so a re-queue matches one in flight (`0x6143f0`); the count is zeroed after the loop, dropping what the handlers queued (`0x6147a3`).
    let Some(events) = world
        .get_non_send_resource::<UniqueSignals>()
        .map(|s| s.0.clone())
    else {
        return;
    };
    super::safe_world::lend(world, |script| {
        for event in events {
            script.fire_event(event, Vec::new());
        }
    });
    world.non_send_resource_mut::<UniqueSignals>().clear();
}

/// The disconnect runs the game UI's shutdown, which empties the unique signals (`0x52902f`).
pub(crate) fn clear_unique_signals(
    In(_): In<benilla_protocol::SessionEvent>,
    mut signals: NonSendMut<UniqueSignals>,
) {
    signals.clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A verb's in-call write of the pet's words (the drag, `0x5d5c3e`) and the owner's own write
    /// each mark the bar changed for a later reader in the frame, and no write marks nothing.
    #[test]
    fn a_verbs_write_of_the_pet_bar_is_seen_changed_by_a_later_reader() {
        use crate::ui_pet::PetBar;
        #[derive(Resource, Default)]
        struct Step(u8, Option<bool>);
        let mut world = World::new();
        world.init_resource::<PetBar>();
        world.init_resource::<Step>();
        let mut schedule = Schedule::default();
        schedule.add_systems(
            (
                |world: &mut World| {
                    if world.resource::<Step>().0 == 1 {
                        let wrote = with_world_view(world, |l| l.pet_place_action(&[(0, 0)]));
                        assert!(wrote);
                    }
                },
                |mut bar: ResMut<PetBar>, step: Res<Step>| {
                    if step.0 == 2 {
                        let _ = &mut bar.spells;
                    }
                },
                |bar: Res<PetBar>, mut step: ResMut<Step>| step.1 = Some(bar.is_changed()),
            )
                .chain(),
        );
        let mut seen = Vec::new();
        for n in 0..4 {
            world.resource_mut::<Step>().0 = n;
            schedule.run(&mut world);
            seen.push(world.resource::<Step>().1);
        }
        assert_eq!(seen, [Some(true), Some(true), Some(true), Some(false)]);
    }

    /// `UnstablePet`'s gate with no player object refuses, as its lookup's miss returns (`0x5a1b0b`).
    #[test]
    fn unstable_pet_refuses_with_no_player_object() {
        let mut world = World::new();
        assert!(with_world_view(&mut world, |l| l.charm_or_summon()));
    }
}
